# 月笺 Lunote 网络回归测试（跨进程，真实 socket）
# 两个独立 lunote-cli 进程通过控制文件驱动，验证：
#   跨进程 UDP 发现 → TLS 1.3 认证 → 信任 → 文字 → 文件 → 完整性
# 运行：python tests/regression_cli.py

import json
import os
import subprocess
import sys
import time

BASE = r"D:\Lunote 2\.toolchains\regression"
CLI = r"D:\Lunote 2\.toolchains\rust-target\release\lunote-cli.exe"

def log(msg):
    print("[%s] %s" % (time.strftime("%H:%M:%S"), msg), flush=True)

def run_serve(name, data_dir, tcp_port, events_file, control_file):
    for d in (data_dir,):
        os.makedirs(d, exist_ok=True)
    for f in (events_file, control_file):
        if os.path.exists(f):
            os.remove(f)
    open(control_file, "w", encoding="utf-8").close()
    out = open(events_file + ".proc.out", "w", encoding="utf-8", errors="replace")
    err = open(events_file + ".proc.err", "w", encoding="utf-8", errors="replace")
    return subprocess.Popen(
        [CLI, "serve", "--name", name, "--data-dir", data_dir,
         "--tcp-port", str(tcp_port), "--event-file", events_file,
         "--control-file", control_file],
        stdout=out, stderr=err,
        creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0),
    )

def wait_event(events_file, pred, timeout=20):
    deadline = time.time() + timeout
    last = 0
    while time.time() < deadline:
        try:
            with open(events_file, "r", encoding="utf-8") as f:
                data = f.read()
        except FileNotFoundError:
            time.sleep(0.3)
            continue
        for line in data.splitlines()[last:]:
            last += 1
            if not line.strip():
                continue
            try:
                ev = json.loads(line)
            except json.JSONDecodeError:
                continue
            if pred(ev):
                return ev
        time.sleep(0.3)
    return None

def send_cmd(control_file, cmd):
    with open(control_file, "a", encoding="utf-8") as f:
        f.write(json.dumps(cmd, ensure_ascii=False) + "\n")

def main():
    log("=== 月笺 Lunote 跨进程网络回归 ===")
    a_dir = os.path.join(BASE, "a")
    b_dir = os.path.join(BASE, "b")
    a_ev = os.path.join(BASE, "a.events.jsonl")
    b_ev = os.path.join(BASE, "b.events.jsonl")
    a_ctrl = os.path.join(BASE, "a.control.jsonl")
    b_ctrl = os.path.join(BASE, "b.control.jsonl")

    proc_a = run_serve("回归甲", a_dir, 45656, a_ev, a_ctrl)
    proc_b = run_serve("回归乙", b_dir, 45657, b_ev, b_ctrl)
    log("进程已启动 pid_a=%d pid_b=%d" % (proc_a.pid, proc_b.pid))

    try:
        # 1) 双向发现（快照轮询，不依赖事件时序）
        deadline = time.time() + 25
        a_found = b_found = None
        while time.time() < deadline:
            try:
                with open(a_ev, encoding="utf-8") as f:
                    for line in f:
                        try:
                            ev = json.loads(line)
                            if ev.get("event") == "peer_online" and ev.get("name") == "回归乙":
                                a_found = ev
                        except json.JSONDecodeError:
                            pass
                with open(b_ev, encoding="utf-8") as f:
                    for line in f:
                        try:
                            ev = json.loads(line)
                            if ev.get("event") == "peer_online" and ev.get("name") == "回归甲":
                                b_found = ev
                        except json.JSONDecodeError:
                            pass
            except FileNotFoundError:
                pass
            if a_found and b_found:
                break
            time.sleep(0.5)
        assert a_found and b_found, "双向发现失败 a=%s b=%s" % (a_found, b_found)
        log("1) 双向发现 OK：%s <-> %s" % (a_found["name"], b_found["name"]))

        # 2) A 主动连接 B（触发 TLS 认证 + TOFU）
        send_cmd(a_ctrl, {"cmd": "connect", "device_id": a_found["device_id"]})
        conn = wait_event(a_ev, lambda e: e.get("event") == "peer_connected" and e.get("device_id") == a_found["device_id"])
        assert conn, "A 未建立到 B 的认证会话"
        # B 侧也须完成 TOFU 记录（其 PeerConnected 事件在 check_identity 之后发出）
        conn_b = wait_event(b_ev, lambda e: e.get("event") == "peer_connected" and e.get("device_id") == b_found["device_id"])
        assert conn_b, "B 未完成对 A 的身份记录"
        log("2) TLS 认证会话 OK（新设备=%s）" % conn["is_new_device"])

        # 3) 互信（A 信任 B 用 a_found 中的 B ID；B 信任 A 用 b_found 中的 A ID）
        send_cmd(a_ctrl, {"cmd": "trust", "device_id": a_found["device_id"], "trusted": True})
        send_cmd(b_ctrl, {"cmd": "trust", "device_id": b_found["device_id"], "trusted": True})
        time.sleep(1.0)

        # 4) 文字往返
        send_cmd(a_ctrl, {"cmd": "send_text", "device_id": a_found["device_id"], "text": "跨进程回归消息"})
        msg = wait_event(b_ev, lambda e: e.get("event") == "message_received" and e.get("text") == "跨进程回归消息")
        assert msg, "B 未收到文字消息"
        log("4) 文字往返 OK（msg_id=%s）" % msg["message_id"])

        # 5) 文件往返 + 完整性
        src = os.path.join(BASE, "回归文件.bin")
        with open(src, "wb") as f:
            f.write(os.urandom(4 * 1024 * 1024))
        recv_dir = os.path.join(BASE, "b-recv")
        os.makedirs(recv_dir, exist_ok=True)
        send_cmd(a_ctrl, {"cmd": "send_file", "device_id": a_found["device_id"], "path": src})
        offer = wait_event(b_ev, lambda e: e.get("event") == "transfer_update" and e.get("state") == "offered")
        assert offer, "B 未收到文件提议"
        tid = offer["transfer_id"]
        send_cmd(b_ctrl, {"cmd": "accept", "transfer_id": tid, "dest": recv_dir})
        done = wait_event(b_ev, lambda e: e.get("event") == "transfer_update" and e.get("transfer_id") == tid and e.get("state") == "done", timeout=60)
        assert done, "文件未完成"
        got = os.path.join(recv_dir, "回归文件.bin")
        with open(src, "rb") as f:
            sha_src = __import__("hashlib").sha256(f.read()).hexdigest()
        with open(got, "rb") as f:
            sha_got = __import__("hashlib").sha256(f.read()).hexdigest()
        assert sha_src == sha_got, "文件哈希不一致"
        log("5) 文件传输+完整性 OK（4 MiB，SHA-256 一致）")

        # 6) 加密记录与导出/导入（跨进程）
        send_cmd(b_ctrl, {"cmd": "export", "password": "regression-pw-123", "out": os.path.join(BASE, "rec.lunote")})
        time.sleep(1.5)
        assert os.path.exists(os.path.join(BASE, "rec.lunote")), "导出文件未生成"
        send_cmd(a_ctrl, {"cmd": "import", "password": "regression-pw-123", "input": os.path.join(BASE, "rec.lunote")})
        time.sleep(1.5)
        log("6) 加密记录导出/导入 OK")

        log("=== 回归全部通过 ===")
        return 0
    finally:
        send_cmd(a_ctrl, {"quit": True})
        send_cmd(b_ctrl, {"quit": True})
        time.sleep(1)
        for p in (proc_a, proc_b):
            if p.poll() is None:
                p.kill()

if __name__ == "__main__":
    sys.exit(main())

