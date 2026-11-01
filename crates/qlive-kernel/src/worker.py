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

import json
import sys
import time

from jupyter_client.manager import start_new_kernel


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
                msg_id = kc.execute(req["code"])
                kc.get_shell_msg(timeout=600)
                outputs, count = collect(kc, msg_id)
                resp = {
                    "id": rid,
                    "ok": True,
                    "outputs": outputs,
                    "execution_count": count,
                    "ms": (time.perf_counter() - started) * 1000,
                }
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
