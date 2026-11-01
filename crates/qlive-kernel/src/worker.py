"""qlive kernel worker: one JSON object per line, in and out.

Holds a single Jupyter kernel for the life of the process. That is the whole point:
skipping a cell is only sound while the variables it defined are still in the kernel,
so the process that owns the kernel has to outlive the render.

  -> {"id":1,"op":"start"}
  <- {"id":1,"ok":true,"ms":1407.8}
  -> {"id":2,"op":"execute","code":"x = 21"}
  <- {"id":2,"ok":true,"outputs":[...],"execution_count":1,"ms":6.4}
  -> {"id":3,"op":"shutdown"}

Outputs are already nbformat-shaped, so they can be written straight into a notebook.

With `--serve STATE IDLE LABEL` the same protocol runs on a loopback socket behind a
token, so the kernel outlives the qlive that started it; see `serve`.
"""

import ast
import json
import sys
import time

from jupyter_client.manager import start_new_kernel

# Calls that make a cell's effect on the namespace unknowable.
OPAQUE_CALLS = {"exec", "eval", "globals", "locals", "vars", "__import__", "get_ipython", "setattr", "delattr"}

# Methods that mutate their receiver. Deliberately a list: treating every method call
# as a mutation would make `df.head()` a definer of `df`, so every later cell using
# `df` would re-run — the over-invalidation this analysis exists to remove. Mutation
# through other methods, or through function arguments (`np.random.shuffle(data)`),
# is not tracked; `#| depends:` covers those.
MUTATING_METHODS = {
    "append", "extend", "insert", "remove", "pop", "popitem", "clear", "sort", "reverse",
    "update", "add", "discard", "setdefault", "fill", "resize", "put", "itemset",
    "__setitem__", "__delitem__",
}


class Analyzer(ast.NodeVisitor):
    """Which module-level names a cell defines and reads, in evaluation order.

    Scope-aware: lambda parameters and comprehension variables stay local; a
    function body's global reads count as reads of the defining cell (they happen
    at call time, wherever that is); `x += 1` reads before it writes; `a[i] = v`,
    `a.b = v` and method calls on a non-module name mutate it, so they read *and*
    redefine it. Over-approximating is safe (an extra re-run); missing a read is not.
    """

    def __init__(self):
        self.bound = set()      # module-level names bound so far in this cell
        self.defines = set()
        self.uses = set()
        self.deferred = set()   # globals read inside function bodies
        self.modules = set()
        self.scopes = []        # local names of enclosing lambdas/comprehensions/functions/classes
        self.declared_global = []
        self.in_function = 0
        self.opaque = False

    # -- names ------------------------------------------------------------
    def load(self, name):
        for scope in reversed(self.scopes):
            if name in scope:
                return
        if self.in_function:
            self.deferred.add(name)
        elif name not in self.bound:
            self.uses.add(name)

    def store(self, name):
        if self.scopes and not (self.declared_global and name in self.declared_global[-1]):
            self.scopes[-1].add(name)
            return
        self.bound.add(name)
        self.defines.add(name)

    def mutate(self, name):
        if name in self.modules:
            return
        self.load(name)
        if not self.scopes:
            self.defines.add(name)
            self.bound.add(name)
        elif self.in_function:
            # Mutating a global inside a function: count it as defined here.
            self.defines.add(name)

    def visit_Name(self, node):
        if isinstance(node.ctx, ast.Load):
            self.load(node.id)
        else:
            self.store(node.id)

    # -- assignment ---------------------------------------------------------
    def target(self, node):
        if isinstance(node, ast.Name):
            self.store(node.id)
        elif isinstance(node, (ast.Tuple, ast.List)):
            for elt in node.elts:
                self.target(elt)
        elif isinstance(node, ast.Starred):
            self.target(node.value)
        elif isinstance(node, (ast.Attribute, ast.Subscript)):
            if isinstance(node, ast.Subscript):
                self.visit(node.slice)
            base = node.value
            while isinstance(base, (ast.Attribute, ast.Subscript)):
                if isinstance(base, ast.Subscript):
                    self.visit(base.slice)
                base = base.value
            if isinstance(base, ast.Name):
                self.mutate(base.id)
            else:
                self.visit(base)

    def visit_Assign(self, node):
        self.visit(node.value)
        for t in node.targets:
            self.target(t)

    def visit_AugAssign(self, node):
        self.visit(node.value)
        if isinstance(node.target, ast.Name):
            self.load(node.target.id)
            self.store(node.target.id)
        else:
            self.target(node.target)

    def visit_AnnAssign(self, node):
        if node.value is not None:
            self.visit(node.value)
        self.target(node.target)

    def visit_NamedExpr(self, node):
        self.visit(node.value)
        self.target(node.target)

    def visit_Delete(self, node):
        for t in node.targets:
            self.target(t)

    def visit_For(self, node):
        self.visit(node.iter)
        self.target(node.target)
        for stmt in node.body + node.orelse:
            self.visit(stmt)

    visit_AsyncFor = visit_For

    def visit_With(self, node):
        for item in node.items:
            self.visit(item.context_expr)
            if item.optional_vars is not None:
                self.target(item.optional_vars)
        for stmt in node.body:
            self.visit(stmt)

    visit_AsyncWith = visit_With

    def visit_ExceptHandler(self, node):
        if node.type is not None:
            self.visit(node.type)
        if node.name:
            self.store(node.name)
        for stmt in node.body:
            self.visit(stmt)

    # -- imports ------------------------------------------------------------
    def visit_Import(self, node):
        for alias in node.names:
            name = alias.asname or alias.name.split(".")[0]
            self.modules.add(name)
            self.store(name)

    def visit_ImportFrom(self, node):
        for alias in node.names:
            if alias.name == "*":
                self.opaque = True
            else:
                self.store(alias.asname or alias.name)

    # -- calls --------------------------------------------------------------
    def visit_Call(self, node):
        func = node.func
        if isinstance(func, ast.Name) and func.id in OPAQUE_CALLS:
            self.opaque = True
        inplace = any(
            k.arg == "inplace" and isinstance(k.value, ast.Constant) and k.value.value is True
            for k in node.keywords
        )
        if isinstance(func, ast.Attribute) and (func.attr in MUTATING_METHODS or inplace):
            base = func.value
            while isinstance(base, (ast.Attribute, ast.Subscript)):
                base = base.value
            if isinstance(base, ast.Name):
                self.mutate(base.id)
        self.generic_visit(node)

    # -- scopes -------------------------------------------------------------
    def visit_FunctionDef(self, node):
        for d in node.decorator_list:
            self.visit(d)
        for default in node.args.defaults + [d for d in node.args.kw_defaults if d is not None]:
            self.visit(default)
        self.store(node.name)
        params = {a.arg for a in node.args.posonlyargs + node.args.args + node.args.kwonlyargs}
        if node.args.vararg:
            params.add(node.args.vararg.arg)
        if node.args.kwarg:
            params.add(node.args.kwarg.arg)
        self.scopes.append(params)
        self.declared_global.append(set())
        self.in_function += 1
        for stmt in node.body:
            self.visit(stmt)
        self.in_function -= 1
        self.declared_global.pop()
        self.scopes.pop()

    visit_AsyncFunctionDef = visit_FunctionDef

    def visit_Global(self, node):
        if self.declared_global:
            self.declared_global[-1].update(node.names)

    def visit_ClassDef(self, node):
        for d in node.decorator_list + node.bases + [k.value for k in node.keywords]:
            self.visit(d)
        self.scopes.append(set())
        self.declared_global.append(set())
        for stmt in node.body:   # a class body runs now
            self.visit(stmt)
        self.declared_global.pop()
        self.scopes.pop()
        self.store(node.name)

    def visit_Lambda(self, node):
        for default in node.args.defaults + [d for d in node.args.kw_defaults if d is not None]:
            self.visit(default)
        params = {a.arg for a in node.args.posonlyargs + node.args.args + node.args.kwonlyargs}
        self.scopes.append(params)
        self.in_function += 1
        self.visit(node.body)
        self.in_function -= 1
        self.scopes.pop()

    def comprehension(self, node, parts):
        self.visit(node.generators[0].iter)   # evaluated in the enclosing scope
        self.scopes.append(set())
        for i, gen in enumerate(node.generators):
            if i:
                self.visit(gen.iter)
            self.target(gen.target)
            for cond in gen.ifs:
                self.visit(cond)
        for part in parts:
            self.visit(part)
        self.scopes.pop()

    def visit_ListComp(self, node):
        self.comprehension(node, [node.elt])

    visit_SetComp = visit_ListComp
    visit_GeneratorExp = visit_ListComp

    def visit_DictComp(self, node):
        self.comprehension(node, [node.key, node.value])


def analyze(code):
    """{"defines", "uses", "opaque"} for one cell. Magics and syntax errors are opaque."""
    try:
        tree = ast.parse(code)
    except SyntaxError:
        return {"defines": [], "uses": [], "opaque": True}
    a = Analyzer()
    for stmt in tree.body:
        a.visit(stmt)
    uses = a.uses | {n for n in a.deferred if n not in a.defines}
    return {"defines": sorted(a.defines), "uses": sorted(uses), "opaque": a.opaque}


def collect(kc, msg_id, timeout=600):
    """Gather nbformat outputs for one execution, until the kernel reports idle."""
    outputs = []
    execution_count = None
    while True:
        msg = kc.get_iopub_msg(timeout=timeout)
        if msg.get("parent_header", {}).get("msg_id") != msg_id:
            continue
        kind = msg["msg_type"]
        content = msg["content"]
        if kind == "status" and content.get("execution_state") == "idle":
            break
        elif kind == "stream":
            outputs.append(
                {"output_type": "stream", "name": content["name"], "text": content["text"]}
            )
        elif kind in ("display_data", "update_display_data"):
            outputs.append(
                {
                    "output_type": "display_data",
                    "data": content["data"],
                    "metadata": content.get("metadata", {}),
                }
            )
        elif kind == "execute_result":
            execution_count = content.get("execution_count")
            outputs.append(
                {
                    "output_type": "execute_result",
                    "data": content["data"],
                    "metadata": content.get("metadata", {}),
                    "execution_count": execution_count,
                }
            )
        elif kind == "error":
            outputs.append(
                {
                    "output_type": "error",
                    "ename": content["ename"],
                    "evalue": content["evalue"],
                    "traceback": content["traceback"],
                }
            )
        elif kind == "execute_input":
            execution_count = content.get("execution_count", execution_count)
    return outputs, execution_count


def run_quietly(kc, code):
    msg_id = kc.execute(code, store_history=False, allow_stdin=False)
    kc.get_shell_msg(timeout=600)
    return collect(kc, msg_id)


def handle(req, st):
    """One request against the worker state `st`; returns (reply, stop)."""
    rid, op = req.get("id"), req.get("op")
    km, kc = st.get("km"), st.get("kc")
    if op == "start":
        started = time.perf_counter()
        st["km"], st["kc"] = start_new_kernel(kernel_name=req.get("kernel", "python3"))
        return {"id": rid, "ok": True, "ms": (time.perf_counter() - started) * 1000}, False
    if op == "ping":
        return {"id": rid, "ok": True, "started": kc is not None}, False
    if op == "stash":
        # The client's bookkeeping about this kernel's variables. Kept here so it lives
        # exactly as long as the variables do.
        st["stash"] = req.get("state")
        return {"id": rid, "ok": True}, False
    if op == "unstash":
        return {"id": rid, "ok": True, "state": st.get("stash")}, False
    if op == "execute":
        if kc is None:
            raise RuntimeError("kernel not started")
        started = time.perf_counter()
        # allow_stdin=False, as nbclient (and so Quarto) executes: a cell that
        # asks for input must fail, not hang the preview waiting for a keypress.
        msg_id = kc.execute(req["code"], allow_stdin=False)
        kc.get_shell_msg(timeout=600)
        outputs, count = collect(kc, msg_id)
        return {
            "id": rid,
            "ok": True,
            "outputs": outputs,
            "execution_count": count,
            "ms": (time.perf_counter() - started) * 1000,
        }, False
    if op == "setup":
        # Quarto's own kernel setup: share/jupyter/lang/python/setup.py, filled
        # with str.format exactly as notebook.py's nb_language_cell does, and
        # run with store_history=False so document cells still count from 1.
        if kc is None:
            raise RuntimeError("kernel not started")
        import base64

        args = dict(req.get("args", {}))
        args["run_path"] = base64.b64encode(
            args.get("run_path", "").encode("utf-8")
        ).decode("utf-8")
        with open(req["path"], "r", encoding="utf-8") as f:
            source = f.read().format(**args)

        started = time.perf_counter()
        failures = []
        env = req.get("env") or {}
        for code in (
            # set_env_vars sets these before the kernel starts; same effect here.
            "import os\nos.environ.update(" + json.dumps(env) + ")\n" if env else "",
            source,
        ):
            if not code:
                continue
            # setup.py runs `%reset`, which asks "Proceed (y/[n])?" on stdin and
            # waits forever unless stdin is disallowed; IPython then treats the
            # missing stdin as yes. That reset is also why Quarto gives every
            # document a fresh kernel — so setup runs once per session here,
            # before the first document, never between documents.
            outputs, _ = run_quietly(kc, code)
            failures += [o for o in outputs if o["output_type"] == "error"]
        if failures:
            raise RuntimeError(
                f"setup failed: {failures[0]['ename']}: {failures[0]['evalue']}"
            )
        return {"id": rid, "ok": True, "ms": (time.perf_counter() - started) * 1000}, False
    if op == "analyze":
        # Parsed here, in the worker, never executed in the kernel.
        return {"id": rid, "ok": True, "cells": [analyze(c) for c in req["codes"]]}, False
    if op == "forget":
        # Delete names whose only definer is gone, so their readers fail the
        # way they would in a fresh run instead of seeing a stale value.
        if kc is None:
            raise RuntimeError("kernel not started")
        run_quietly(kc, "for _qlive_n in " + json.dumps(req["names"]) + ":\n"
                        "    globals().pop(_qlive_n, None)\ndel _qlive_n\n")
        return {"id": rid, "ok": True}, False
    if op == "restart":
        if km is None:
            raise RuntimeError("kernel not started")
        km.restart_kernel(now=True)
        st["kc"] = km.client()
        st["kc"].start_channels()
        st["kc"].wait_for_ready(timeout=120)
        st["stash"] = None
        return {"id": rid, "ok": True}, False
    if op == "shutdown":
        if km is not None:
            km.shutdown_kernel(now=True)
            st["km"] = st["kc"] = None
        return {"id": rid, "ok": True}, True
    raise RuntimeError(f"unknown op: {op}")


def handle_line(line, st):
    line = line.strip()
    if not line:
        return None, False
    try:
        req = json.loads(line)
    except Exception as e:
        return {"ok": False, "err": f"bad json: {e}"}, False
    try:
        return handle(req, st)
    except Exception as e:
        return {"id": req.get("id"), "ok": False, "err": f"{type(e).__name__}: {e}"}, False


def main():
    st = {}
    for line in sys.stdin:
        resp, stop = handle_line(line, st)
        if resp is not None:
            print(json.dumps(resp), flush=True)
        if stop:
            return


def serve_client(conn, st, token, owner):
    """Authenticate one connection and, if the kernel is free, answer it until it
    closes. Returns True when it asked the daemon to shut down."""
    import hmac

    rf = conn.makefile("r", encoding="utf-8")
    wf = conn.makefile("w", encoding="utf-8")
    try:
        conn.settimeout(5)
        try:
            hello = json.loads(rf.readline() or "{}")
            ok = hmac.compare_digest(str(hello.get("token", "")).encode(), token.encode())
        except ValueError:
            ok = False
        if not ok:
            return False  # a status probe, or a stranger
        conn.settimeout(None)
        if not owner.acquire(blocking=False):
            wf.write(json.dumps({"ok": False, "err": "busy"}) + "\n")
            wf.flush()
            return False
        try:
            wf.write(json.dumps({"ok": True}) + "\n")
            wf.flush()
            for line in rf:
                resp, stop = handle_line(line, st)
                if resp is not None:
                    wf.write(json.dumps(resp) + "\n")
                    wf.flush()
                if stop:
                    return True
            return False
        finally:
            st["last_client"] = time.monotonic()
            owner.release()
    finally:
        for f in (wf, rf):
            try:
                f.close()
            except (OSError, ValueError):
                pass


def serve(state_file, idle_seconds, label):
    """Daemon mode: the same protocol on a loopback socket, so the kernel and its
    variables survive the qlive process that started them.

    Anyone who can connect could run code, so a client's first line must carry the
    256-bit token written to `state_file`, which lives in the user's own cache
    directory. Connections are always accepted, each on its own thread, so status
    probes never queue up behind a busy client; only one authenticated client uses
    the kernel at a time, and a second one is told it is busy. Exits, shutting the
    kernel down, after `idle_seconds` without an authenticated client.
    """
    import os
    import secrets
    import socket
    import threading

    token = secrets.token_hex(32)
    server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server.bind(("127.0.0.1", 0))
    server.listen(16)
    server.settimeout(1.0)
    temporary = state_file + ".tmp"
    with open(temporary, "w", encoding="utf-8") as f:
        json.dump({"port": server.getsockname()[1], "token": token, "pid": os.getpid(),
                   "label": label, "started": time.time()}, f)
    os.replace(temporary, state_file)

    st = {"last_client": time.monotonic()}
    owner = threading.Lock()
    done = threading.Event()

    def client(conn):
        # A client that vanishes mid-cell must not take the kernel with it: every
        # socket error, including the flush when the files close, ends only this
        # connection.
        try:
            if serve_client(conn, st, token, owner):
                done.set()
        except (OSError, ValueError):
            pass
        finally:
            try:
                conn.close()
            except OSError:
                pass

    try:
        while not done.is_set():
            try:
                conn, _ = server.accept()
            except socket.timeout:
                idle = time.monotonic() - st["last_client"]
                if not owner.locked() and idle > idle_seconds:
                    break
                continue
            threading.Thread(target=client, args=(conn,), daemon=True).start()
    finally:
        if st.get("km") is not None:
            st["km"].shutdown_kernel(now=True)
        try:
            with open(state_file, encoding="utf-8") as f:
                if json.load(f).get("pid") == os.getpid():
                    os.remove(state_file)
        except Exception:
            pass


def spawn_daemon(state_file, idle_seconds, label):
    """Start `serve` detached and return at once. The daemon's parent is gone by the
    time anyone kills qlive's process tree, so it survives that too."""
    import subprocess

    args = [sys.executable, __file__, "--serve", state_file, str(idle_seconds), label]
    common = {"stdin": subprocess.DEVNULL, "stdout": subprocess.DEVNULL,
              "stderr": subprocess.DEVNULL, "close_fds": True}
    if sys.platform == "win32":
        # A hidden console of its own, not none: with DETACHED_PROCESS every console
        # program the daemon starts, ipykernel included, would open a visible window.
        no_window, group, breakaway = 0x08000000, 0x00000200, 0x01000000
        try:
            subprocess.Popen(args, creationflags=no_window | group | breakaway, **common)
        except OSError:
            # Some job objects forbid breakaway; with this process gone, the daemon
            # still survives a tree kill.
            subprocess.Popen(args, creationflags=no_window | group, **common)
    else:
        subprocess.Popen(args, start_new_session=True, **common)


if __name__ == "__main__":
    if len(sys.argv) >= 5 and sys.argv[1] == "--serve":
        serve(sys.argv[2], float(sys.argv[3]), sys.argv[4])
    elif len(sys.argv) >= 5 and sys.argv[1] == "--spawn-daemon":
        spawn_daemon(sys.argv[2], float(sys.argv[3]), sys.argv[4])
    else:
        main()
