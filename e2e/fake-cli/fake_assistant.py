"""Stand-in for the Claude Code / Codex CLIs in tests: speaks their JSON event formats,
answers from the prompt and the working directory, and logs how it was called."""
import json
import os
import sys
import time


def main(provider):
    args = sys.argv[1:]
    if args in (["auth", "login"], ["login"]):
        return sign_in(provider, args)
    prompt = sys.stdin.read()
    cwd = os.getcwd()
    write_log({"provider": provider, "args": args, "cwd": cwd, "prompt": prompt})

    questions = [l[len("Question: "):] for l in prompt.splitlines() if l.startswith("Question: ")]
    question = questions[-1] if questions else prompt.strip()
    resumed = "--resume" in args if provider == "claude" else "resume" in args
    entries = sorted(e for e in os.listdir(cwd) if not e.startswith("."))
    answer = f"{'(follow-up) ' if resumed else ''}About “{question}”: I can see {', '.join(entries[:3])} in the checkout."

    def emit(event):
        print(json.dumps(event), flush=True)

    if provider == "claude":
        emit({"type": "system", "subtype": "init", "session_id": "fake-claude-session", "cwd": cwd})
        if question == "fail":
            emit({"type": "result", "subtype": "success", "is_error": True, "result": "Something went wrong (fake)"})
            return
        if question == "expired":
            emit({"type": "result", "subtype": "success", "is_error": True, "result": "Failed to authenticate: OAuth session expired and could not be refreshed"})
            return
        for word in answer.split(" "):
            emit({"type": "stream_event", "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": word + " "}}})
        emit({"type": "assistant", "message": {"content": [{"type": "text", "text": answer}]}})
        emit({"type": "result", "subtype": "success", "is_error": False, "result": answer})
    else:
        emit({"type": "thread.started", "thread_id": "fake-codex-thread"})
        emit({"type": "turn.started"})
        if question == "fail":
            emit({"type": "turn.failed", "error": {"message": json.dumps({"type": "error", "status": 400, "error": {"message": "model not available (fake)"}})}})
            sys.exit(1)
        emit({"type": "item.completed", "item": {"id": "item_0", "type": "agent_message", "text": answer}})
        emit({"type": "turn.completed", "usage": {}})


def write_log(entry):
    log = os.environ.get("WISPY_FAKE_LOG")
    if log:
        with open(log, "a") as f:
            f.write(json.dumps(entry) + "\n")


def sign_in(provider, args):
    """Like `claude auth login` / `codex login`: prints the sign-in page (Claude's as a terminal
    hyperlink, Codex's after its localhost callback), then "completes" the sign-in."""
    write_log({"provider": provider, "args": args, "browser": os.environ.get("BROWSER")})
    url = f"https://{provider}.example.test/oauth/authorize?state=fake"
    if provider == "claude":
        print("Opening browser to sign in\u2026")
        print(f"If the browser didn't open, visit: \x1b]8;;{url}\x07{url}\x1b]8;;\x07")
        print("Paste code here if prompted > ", end="", flush=True)
        time.sleep(0.5)
        print("\nLogin successful.", flush=True)
    else:
        print("Starting local login server on http://localhost:1455.")
        print(f"If your browser did not open, navigate to this URL to authenticate:\n\n{url}", flush=True)
        time.sleep(0.5)
        print("Successfully logged in", flush=True)
