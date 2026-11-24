"""Writes a projection with every T3 Code MCP tool, for the timeline comparison.

Usage: python3 -I synthetic.py <projections-dir> <t3_tools.rs>
Starts from the claude_mcp_tool_presentation fixture projection and replaces its rows with,
per tool, two completed calls, a running and a failed one, then a few mixed groups; replies
between them close each group. send_to_user and html_render get the envelopes real calls return.
"""
import copy
import json
import re
import sys

projections, table = sys.argv[1:3]
base = json.load(open(f"{projections}/claude_mcp_tool_presentation__claudeAgent.json"))[0]
rows = base["visibleTurnItems"]
user, tool, reply = rows[0], rows[2], rows[4]
names = re.findall(r'T3ToolDefinition \{ name: "([^"]+)"', open(table).read())
out = [copy.deepcopy(user)]
count = 0


def text_output(value):
    return {"content": [{"type": "text", "text": json.dumps(value)}]}


def add_tool(name, status, tool_input, output, prefix):
    global count
    count += 1
    row = copy.deepcopy(tool)
    item = row["item"]
    item["id"] = f"turn-item:synthetic:tool-{count}"
    row["sourceItemId"] = item["id"]
    item.update(toolName=prefix + name, status=status, input=tool_input, title=None)
    item.pop("toolSource", None)
    item.pop("toolIcon", None)
    if output is None:
        item.pop("output", None)
    else:
        item["output"] = output
    out.append(row)


def add_reply():
    global count
    count += 1
    row = copy.deepcopy(reply)
    item = row["item"]
    item.update(id=f"turn-item:synthetic:reply-{count}", messageId=f"message:synthetic:{count}", text=f"reply {count}")
    row["sourceItemId"] = item["id"]
    out.append(row)


for index, name in enumerate(names):
    prefix = ["mcp__t3-code__", "t3-code.", "mcp__t3_code_direct__"][index % 3]
    pull_request = "pull" in name or "pr_" in name or name.startswith(("merge", "ci_"))
    first = {"threadId": "thread-a", "url": "https://github.com/o/r/pull/12"} if pull_request else {"threadId": "thread-a", "projectId": "p1", "taskId": "t1"}
    completed = text_output({"threadId": "thread-a", "messageId": "m1", "id": "x1"})
    if name == "send_to_user":
        completed = text_output({"sentMessage": "Hello from the agent"})
    if name == "html_render":
        completed = {"structuredContent": {"htmlRender": {"attachmentId": "att-1", "title": "  Report  ", "height": 400}}}
    add_tool(name, "completed", first, completed, prefix)
    add_tool(name, "completed", {"threadId": "thread-b"}, text_output({"threadId": "thread-b", "messageId": "m2"}), prefix)
    add_reply()
    add_tool(name, "running", first, None, prefix)
    add_tool(name, "failed", first, {"isError": True, "content": [{"type": "text", "text": "boom"}]}, prefix)
    add_reply()
for combo in [("t3_thread_read", "t3_thread_send", "delegate_task"), ("link_pull_request", "preview_navigate", "device_screenshot")]:
    for name in combo:
        add_tool(name, "completed", {"threadId": "thread-a"}, text_output({"threadId": "thread-a"}), "mcp__t3-code__")
    add_reply()
for position, row in enumerate(out):
    row["position"] = position
base["visibleTurnItems"] = out
base["turnItems"] = [row["item"] for row in out]
json.dump([base], open(f"{projections}/synthetic__t3_tools.json", "w"))
print(f"synthetic: {len(out)} rows, {len(names)} tools")
