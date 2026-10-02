"""Windows FFI endpoint for android_interop_test.dart; isolated, bounded test data."""
import ctypes
import json
from pathlib import Path
import tempfile
import time


def main():
    root = Path(__file__).resolve().parents[1]
    lib = ctypes.CDLL(str(root / "target/release/lunote_bridge.dll"))
    lib.lunote_create.argtypes = [ctypes.c_char_p]
    lib.lunote_create.restype = ctypes.c_int64
    lib.lunote_call.argtypes = [ctypes.c_int64, ctypes.c_char_p]
    lib.lunote_call.restype = ctypes.c_void_p
    lib.lunote_free_string.argtypes = [ctypes.c_void_p]
    lib.lunote_destroy.argtypes = [ctypes.c_int64]
    with tempfile.TemporaryDirectory(prefix="lunote_interop_pc_") as directory:
        handle = lib.lunote_create(json.dumps({"data_dir": directory,
            "name": "interop-pc-v2", "tcp_port": 45889}).encode())
        assert handle > 0, "Windows core failed to start"

        def call(command, **args):
            pointer = lib.lunote_call(handle, json.dumps({"cmd": command, **args}).encode())
            try:
                value = json.loads(ctypes.string_at(pointer).decode())
            finally:
                lib.lunote_free_string(pointer)
            assert value.get("ok"), value.get("error")
            return value

        try:
            print("READY: Windows endpoint TCP 45889", flush=True)
            deadline = time.monotonic() + 240
            peer = None
            note_id = None
            transfer_id = None
            while time.monotonic() < deadline:
                if peer is None:
                    peer = next((p["device_id"] for p in call("peers")["peers"]
                        if p["name"] == "interop-android-v2" and p["online"]), None)
                    if peer:
                        call("trust", device_id=peer)
                        call("set_note_peers", device_ids=[peer])
                        result = call("save_note", note={"id": "", "title": "Cross-platform locked note",
                            "body": "windows-secret", "locked": None, "clock": {}, "deleted": False,
                            "pinned": True, "group": "Interop", "order": 0, "shielded": True},
                            new_password="interop-password-123")
                        note_id = result["note"]["id"]
                if note_id:
                    note = next(n for n in call("notes")["notes"] if n["id"] == note_id)
                    body = call("unlock_note", note_id=note_id, password="interop-password-123")["body"]
                    if body == "android-edited-secret" and transfer_id is None:
                        assert note["body"] is None and note["locked"] is not None
                        call("send_text", device_id=peer, text="windows-main-message")
                        thread = call("create_thread", device_id=peer, title="Interop temporary")["thread"]
                        call("send_thread_text", device_id=peer, thread_id=thread["id"], text="windows-thread-message")
                        path = Path(directory) / "interop.bin"
                        size = 4 * 1024 * 1024
                        path.write_bytes((bytes(range(251)) * (size // 251 + 1))[:size])
                        transfer_id = call("send_file", device_id=peer, thread_id=thread["id"], path=str(path))["transfer_ids"][0]
                if transfer_id:
                    transfers = call("transfers")["transfers"]
                    records = call("conversations")["conversations"]
                    reply = any(m["text"] == "android-thread-reply" for c in records for m in c["messages"])
                    history = [t for c in records for t in c["transfers"]]
                    done = any(t["transfer_id"] == transfer_id and t["state"] == "done" for t in transfers + history)
                    if reply and done:
                        print("PASS: encrypted note roundtrip, isolated thread reply, 4 MiB verified transfer", flush=True)
                        return
                time.sleep(0.2)
            raise TimeoutError("Android interoperability did not complete in 240 seconds")
        finally:
            lib.lunote_destroy(handle)


if __name__ == "__main__":
    main()
