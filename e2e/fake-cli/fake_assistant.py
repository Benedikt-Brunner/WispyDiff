"""Stand-in for the Claude Code / Codex CLIs in tests: speaks their JSON event formats,
answers from the prompt and the working directory, and logs how it was called."""
import json
import os
import re
import sys
import time
import urllib.error
import urllib.request


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
    if question.startswith("tools:"):
        answer = call_tools(provider, args, json.loads(question[len("tools:"):]))
    if question in ("comments", "comments:wait"):
        answer = comments(provider, args, prompt, question.endswith(":wait"))
    if question == "revise":
        answer = revise(provider, args)
    if question.startswith("say:"):
        answer = question[len("say:"):].replace("\\n", "\n")

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


def call_tools(provider, args, steps):
    """Calls the app's review tools in order (`["wait"]` holds until the test says go) and
    answers with their results, one per line."""
    tools = Tools(provider, args)
    lines = []
    for step in steps:
        if step == ["wait"]:
            wait_for_go()
            continue
        name, arguments = step
        ok, text = tools.call(name, arguments)
        lines.append(f"{name}: {text if ok else 'error: ' + text}")
    return "\n".join(lines)


def comments(provider, args, prompt, wait):
    """Review comments as a CLI would add them: on two changed lines of the first file in the
    diff (with a suggestion), on that whole file, and on a file outside the diff. `wait`: holds
    after the first one until the test says go."""
    path, start = first_change(prompt)
    tools = Tools(provider, args)
    tools.call("add_draft_comment", {"path": path, "start_line": start, "end_line": start + 1, "body": "This could be **simpler**:\n```suggestion\nsimpler();\n```"})
    if wait:
        wait_for_go()
    tools.call("add_draft_comment", {"path": path, "body": "Needs a test."})
    tools.call("add_draft_comment", {"path": "nowhere/Missing.php", "start_line": 3, "body": "Lost."})
    return "I added two comments."


def revise(provider, args):
    """Lists the drafts, rewrites the first one it wrote, deletes the second, and edits one that
    doesn't exist."""
    tools = Tools(provider, args)
    _, listing = tools.call("list_drafts", {})
    mine = []
    for i, line in enumerate(lines := listing.splitlines()):
        if line.startswith("[d") and line.endswith("— by you:"):
            body = [l[2:] for l in lines[i + 1:] if l.startswith("> ")][:1]
            mine.append((line[1:line.index("]")], body[0] if body else ""))
    (first, body), (second, _) = mine[:2]
    tools.call("edit_draft", {"ref": first, "body": f"Revised: {body}"})
    tools.call("delete_draft", {"ref": second})
    tools.call("edit_draft", {"ref": "d99", "body": "Nothing."})
    return "Revised."


class Tools:
    """The app's MCP server, found the way each CLI is told about it: Claude's `--mcp-config`,
    Codex's `-c mcp_servers.wispy={url=…, bearer_token_env_var=…}`."""

    def __init__(self, provider, args):
        if provider == "claude":
            server = json.loads(args[args.index("--mcp-config") + 1])["mcpServers"]["wispy"]
            self.url, self.auth = server["url"], server["headers"]["Authorization"]
        else:
            config = next(a for a in args if a.startswith("mcp_servers.wispy="))
            self.url = re.search(r'url="([^"]+)"', config).group(1)
            self.auth = "Bearer " + os.environ[re.search(r'bearer_token_env_var="([^"]+)"', config).group(1)]
        self.next_id = 0
        self.rpc("initialize", {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "fake", "version": "0"}})
        self.rpc("notifications/initialized", None)
        self.names = [t["name"] for t in self.rpc("tools/list", {})["tools"]]

    def call(self, name, arguments):
        """(ok, text) of a tool call; logged."""
        result = self.rpc("tools/call", {"name": name, "arguments": arguments})
        text = "".join(c["text"] for c in result["content"])
        write_log({"tool": name, "arguments": arguments, "result": text, "isError": result.get("isError", False)})
        return not result.get("isError", False), text

    def rpc(self, method, params):
        message = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            self.next_id += 1
            message.update(id=self.next_id, params=params)
        request = urllib.request.Request(
            self.url,
            data=json.dumps(message).encode(),
            headers={"Content-Type": "application/json", "Accept": "application/json, text/event-stream", "Authorization": self.auth},
        )
        with urllib.request.urlopen(request) as response:
            body = response.read()
        return json.loads(body)["result"] if body else None


def wait_for_go():
    """Holds until the test creates `<log>.go`, so it can change things meanwhile."""
    go = os.environ["WISPY_FAKE_LOG"] + ".go"
    while not os.path.exists(go):
        time.sleep(0.05)
    os.remove(go)


def first_change(prompt):
    """The first file in the diff and the first head line of its first hunk with several lines."""
    path = None
    for line in prompt.splitlines():
        if line.startswith("diff --git a/"):
            path = line.split(" b/", 1)[1]
        elif line.startswith("@@") and path:
            new = line.split("+", 1)[1].split(" ", 1)[0].split(",")
            if len(new) > 1 and int(new[1]) > 1:
                return path, int(new[0])


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
