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
    answer = f"{'(follow-up) ' if resumed else ''}About “{question}”: I can see {', '.join(entries[:3])} in the **checkout**."
    if question == "refs":
        answer = references(prompt, cwd)

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


def references(prompt, cwd):
    """An answer citing code the way the CLIs do: a changed line range of the first file in the
    diff, its bare name, the same file by its absolute path in the checkout (as Codex does), a
    file hidden by the filter, a checkout file outside the diff, and things that look like paths
    or files but aren't (a glob, an identifier with slashes, code), and a list of lines."""
    changed, path, covered = None, None, set()
    for line in prompt.splitlines():
        if line.startswith("diff --git a/"):
            path = line.split(" b/", 1)[1]
        elif line.startswith("@@") and path and (not changed or changed[0] == path):
            new = line.split("+", 1)[1].split(" ", 1)[0].split(",")
            start, count = int(new[0]), int(new[1]) if len(new) > 1 else 1
            covered.update(range(start, start + count))
            if not changed and count > 1:
                changed = (path, start)
    diffed = {l.split(" b/", 1)[1] for l in prompt.splitlines() if l.startswith("diff --git a/")}
    hidden = [l[2:] for l in prompt.split("hid these", 1)[1].split("\n\n", 1)[0].splitlines()[1:]] if "hid these" in prompt else []
    other = next(
        rel
        for root, dirs, files in sorted(os.walk(cwd))
        for rel in sorted(os.path.relpath(os.path.join(root, f), cwd) for f in files)
        if "/" in rel and not rel.startswith(".") and rel not in diffed and rel not in hidden
    )
    path, start = changed
    lines = [
        f"The change is in `{path}:{start}-{start + 1}`, see `{os.path.basename(path)}`",
        f"and `{os.path.join(cwd, path)}:{start}`; it calls `{other}:1` and sets `this.state`.",
        f"The `{os.path.dirname(path)}/*.php` files assert `order/germany-gross-order-import/order-create`.",
    ]
    if hidden:
        lines.append(f"The hidden `{hidden[0]}:1` too.")
    lines.append(f"Same in `{os.path.basename(path)}:{start}, {start + 1}-{start + 2}`.")
    # An unchanged line just above the change (only the side-by-side view has it).
    above = max(n for n in range(1, start) if n not in covered)
    lines.append(f"Unchanged: `{path}:{above}`.")
    return " ".join(lines)


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
