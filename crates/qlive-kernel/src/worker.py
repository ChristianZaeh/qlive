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


def main():
    km = kc = None
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except Exception as e:
            print(json.dumps({"ok": False, "err": f"bad json: {e}"}), flush=True)
            continue

        rid, op = req.get("id"), req.get("op")
        try:
            if op == "start":
                started = time.perf_counter()
                km, kc = start_new_kernel(kernel_name=req.get("kernel", "python3"))
                resp = {"id": rid, "ok": True, "ms": (time.perf_counter() - started) * 1000}
            elif op == "execute":
                if kc is None:
                    raise RuntimeError("kernel not started")
                started = time.perf_counter()
                # allow_stdin=False, as nbclient (and so Quarto) executes: a cell that
                # asks for input must fail, not hang the preview waiting for a keypress.
                msg_id = kc.execute(req["code"], allow_stdin=False)
                kc.get_shell_msg(timeout=600)
                outputs, count = collect(kc, msg_id)
                resp = {
                    "id": rid,
                    "ok": True,
                    "outputs": outputs,
                    "execution_count": count,
                    "ms": (time.perf_counter() - started) * 1000,
                }
            elif op == "setup":
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
                    msg_id = kc.execute(code, store_history=False, allow_stdin=False)
                    kc.get_shell_msg(timeout=600)
                    outputs, _ = collect(kc, msg_id)
                    failures += [o for o in outputs if o["output_type"] == "error"]
                if failures:
                    raise RuntimeError(
                        f"setup failed: {failures[0]['ename']}: {failures[0]['evalue']}"
                    )
                resp = {"id": rid, "ok": True, "ms": (time.perf_counter() - started) * 1000}
            elif op == "analyze":
                # Parsed here, in the worker, never executed in the kernel.
                resp = {"id": rid, "ok": True, "cells": [analyze(c) for c in req["codes"]]}
            elif op == "forget":
                # Delete names whose only definer is gone, so their readers fail the
                # way they would in a fresh run instead of seeing a stale value.
                if kc is None:
                    raise RuntimeError("kernel not started")
                code = "for _qlive_n in " + json.dumps(req["names"]) + ":\n" \
                       "    globals().pop(_qlive_n, None)\ndel _qlive_n\n"
                msg_id = kc.execute(code, store_history=False, allow_stdin=False)
                kc.get_shell_msg(timeout=600)
                collect(kc, msg_id)
                resp = {"id": rid, "ok": True}
            elif op == "restart":
                if km is None:
                    raise RuntimeError("kernel not started")
                km.restart_kernel(now=True)
                kc = km.client()
                kc.start_channels()
                kc.wait_for_ready(timeout=120)
                resp = {"id": rid, "ok": True}
            elif op == "shutdown":
                if km is not None:
                    km.shutdown_kernel(now=True)
                print(json.dumps({"id": rid, "ok": True}), flush=True)
                return
            else:
                raise RuntimeError(f"unknown op: {op}")
        except Exception as e:
            resp = {"id": rid, "ok": False, "err": f"{type(e).__name__}: {e}"}
        print(json.dumps(resp), flush=True)


if __name__ == "__main__":
    main()
