"""Stand-in for the Claude Code / Codex CLIs in tests: speaks their JSON event formats,
answers from the prompt and the working directory, and logs how it was called."""
import json
import os
import sys


def main(provider):
    args = sys.argv[1:]
    prompt = sys.stdin.read()
    cwd = os.getcwd()
    log = os.environ.get("WISPY_FAKE_LOG")
    if log:
        with open(log, "a") as f:
            f.write(json.dumps({"provider": provider, "args": args, "cwd": cwd, "prompt": prompt}) + "\n")

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
            emit({"type": "result", "subtype": "success", "is_error": True, "result": "Failed to authenticate (fake)"})
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
