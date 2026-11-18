#!/usr/bin/env python3
"""假的 agy：集成测试用（tests/serve/）。不联网、不碰真的 ~/.gemini：只在它拿到的 HOME 下建会话文件。

用法：fake_agy.py <怎么演> <agy 的参数…>。测试给每种演法写一个 sh 包装，`exec` 到这里（进程号不变）。
环境：FAKE_RECORD 是记录目录（每个进程写一份 <pid>.json：参数、工作目录、HOME、标准输入的第一行 stdin、收到的每一行
lines、子进程号），FAKE_SAMPLES 是真样本的目录。
可选环境 FAKE_AGY_HOME=<目录>：像主人的包装脚本那样给 agy 换了家目录。有它时，假 agy 拉起的 MCP 服务（读 mcp_config.json 起的那个
子进程）的 HOME 是它，假 agy 自己的会话文件（conversations/<编号>.db*）、转录（brain/<编号>/…）都写在它下面（记录里另有 agy_home）；
mcp_config.json、代理文件照旧从原来的 HOME 读（真脚本里那是软链）。没有这个变量时行为和以前一样。

照真 agy：一行标准输入是一轮，答完接着读下一行，第二轮起不再发 init；标准输入关了就退出。

演法（每一轮都照它演，除非写明只演第几轮）：
  sample:<文件>        照样本原样打出来（编号换成自己的，init.agent 换成 --agent 的）
  junk                 同 sample:real-pong.jsonl，init 后面夹一行不是 JSON 的
  echo                 回 "echo: " 加人那句话的最后 12 个字，中间停两下（几次调用同时跑时交错）
  long                 一段三百万字节的正文
  hang                 起一个 sleep 子进程，然后挂住不动
  stubborn             忽略 SIGTERM，挂住不动
  fail:<原话>          error_message 一步、result ERROR，error 是原话
  noresult             init 以后就退出（退出码 1）
  noresult-after-text  交了一段字以后退出（退出码 1）
  badagent             init.agent 报成别的名字，然后挂住
  noread               不读标准输入就退出
  models-fail          `models` 报没登录、退出码 3
  idle-exit            第一轮照 echo 答完，闲着 0.3 秒后自己退出（记录写 exited）
  fail-second          第一轮照 echo，第二轮 result ERROR
  hang-second          第一轮照 echo，第二轮起一个 sleep 子进程、挂住
  die-second           第一轮照 echo，第二轮交一段字以后退出（退出码 1）
  quit-second          第一轮照 echo，第二轮一行不写就退出
  plan:<名字>          照 PLANS 里的剧本演（带工具的：见下）

带工具：启动时读 $HOME/.gemini/config/mcp_config.json，有 mcpServers.miyu 就照它的 command、args 起 MCP 服务（环境变量原样
传下去，和假 agy 同一个进程组，所以 SIGTERM、SIGKILL 一起收），照真 agy 1.4.0 的握手顺序握手（tests/mcp/handshake.rs），把 tools/list
的结果写到 $FAKE_RECORD/<pid>.tools（不叫 .json：records() 只认 .json 的记录）。读 MCP 服务的回话在单独的线程里，主线程等回话时
照样能被信号杀掉。拉不拉 MCP 服务只看 mcp_config.json 里有没有 miyu 条目，和环境变量无关（照真 agy：启动时把登记着的每个服务都拉起来问 tools/list）。记录里另有 mcp_pid、bridge_env（环境里有哪几个桥的变量）、calls（发出的 tools/call）、replies（收到的回话）。

剧本（PLANS）：名字 → 几个进程各自的演法（第几个起的进程用第几份，多了的用最后一份）→ 每一轮（标准输入的一行）的步子（一轮
的步子演完发 result；多出来的轮用最后一份）。工具结果回来是同一轮接着演，不新开一轮。步子：
  ("text", 字)                      一步正文：ACTIVE 带字，DONE 带用量
  ("echo",)                         同 echo 演法
  ("call", 名, 参数)                工具步 ACTIVE 再 tools/call，等回话（记进 replies），再发 DONE
  ("call-mcp-first", 名, 参数)      先 tools/call，隔一会儿才发 ACTIVE，再等回话、发 DONE
  ("callnw", 名, 参数)              ACTIVE 再 tools/call，不等回话
  ("active", 名, 参数)              只发 ACTIVE，不调
  ("mcp-only", 名, 参数)            只调 tools/call（不发 ACTIVE），等回话
  ("speak",)                        等到 $FAKE_RECORD/speak 这个文件出现，再自己发那一步的 ERROR（MCP 调用超时）、一段正文、result，
                                    然后往标准输出塞一兆字节的垃圾行（管道只有 64 KiB：写完说明前面的行扩展都读走了），记 spoke，挂住
  ("hold",)                         挂住不动
  ("chunks", [字, …])               一步正文：每段字一条 ACTIVE，最后 DONE（带用量）
  ("native", 名, 参数, 怎么演, 字, 延迟)  agy 自己跑的原生工具（不带 mcp_miyu_ 前缀），延迟是写转录晚几秒（缺省 0）：
      "transcript"  ACTIVE、DONE（不带 output），延迟以后往转录里写一行（照真样本：两行时间加字）
      "silent"      ACTIVE、DONE，转录里一直不写
      "output"      ACTIVE、DONE 带 output=字；转录里另写一行别的字（证明流里的优先）
      "error"       ACTIVE、ERROR（tool_info.error.message 是字）
      "done-only"   只有 DONE（不带 output），转录里写一行
"""

import json
import os
import signal
import subprocess
import sys
import threading
import time
import uuid

sys.dont_write_bytecode = True  # 别在 tests/ 下留 __pycache__
import fake_agy_plans  # noqa: E402（和本文件同一个目录）

MODE = sys.argv[1]
ARGS = sys.argv[2:]
RECORD = os.environ["FAKE_RECORD"]
SAMPLES = os.environ["FAKE_SAMPLES"]
USAGE = {"input_tokens": 10, "output_tokens": 3, "thinking_tokens": 1, "cache_read_tokens": 7, "total_tokens": 13}

if ARGS[:1] == ["models"]:
    if MODE == "models-fail":
        sys.stderr.write("Error: not logged into Antigravity\n")
        sys.exit(3)
    print("Fetching available models...")
    print("gemini-3.8-flash-low\tGemini 3.8 Flash (Low)")
    print("claude-opus-5-5-high\tClaude Opus 5.5 (High)")
    sys.exit(0)


def option(name):
    return ARGS[ARGS.index(name) + 1]


AGENT = option("--agent")
CONVERSATION = str(uuid.uuid4())
HOME = os.environ["HOME"]
AGY_HOME = os.environ.get("FAKE_AGY_HOME") or HOME  # 会话文件、转录写在这里；配置从 HOME 读
record = {"argv": ARGS, "cwd": os.getcwd(), "home": HOME, "pid": os.getpid(), "conversation": CONVERSATION}


def save():
    path = os.path.join(RECORD, "%d.json" % os.getpid())
    with open(path + ".tmp", "w") as file:
        json.dump(record, file)
    os.rename(path + ".tmp", path)


if AGY_HOME != HOME:
    record["agy_home"] = AGY_HOME
# agy 记下的这段对话：扩展收尾时要删掉。
base = os.path.join(AGY_HOME, ".gemini", "antigravity-cli")
os.makedirs(os.path.join(base, "conversations"), exist_ok=True)
for suffix in (".db", ".db-shm", ".db-wal"):
    with open(os.path.join(base, "conversations", CONVERSATION + suffix), "w") as file:
        file.write("x")
os.makedirs(os.path.join(base, "brain", CONVERSATION, ".system_generated", "logs"), exist_ok=True)

if MODE == "noread":
    save()
    sys.exit(0)
if MODE == "stubborn":
    signal.signal(signal.SIGTERM, signal.SIG_IGN)


def out(event):
    sys.stdout.write(json.dumps(event) + "\n")
    sys.stdout.flush()


class Mcp:
    """假 agy 里的 MCP 客户端：起 MCP 服务、握手、tools/list、tools/call。"""

    def __init__(self, command, args):
        env = os.environ.copy()
        if AGY_HOME != HOME:
            env["HOME"] = AGY_HOME  # 包装脚本给 agy 换了家目录：它拉起的 MCP 服务继承的就是这个
        self.proc = subprocess.Popen(
            [command] + args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env, text=True, bufsize=1
        )
        self.replies = {}
        self.cond = threading.Condition()
        self.next_id = 0
        threading.Thread(target=self.read, daemon=True).start()

    def read(self):
        for line in self.proc.stdout:
            try:
                message = json.loads(line)
            except ValueError:
                continue
            with self.cond:
                self.replies[message.get("id")] = message
                self.cond.notify_all()

    def send(self, message):
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()

    def request(self, method, params):
        self.next_id += 1
        self.send({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
        return self.next_id

    def notify(self, method):
        self.send({"jsonrpc": "2.0", "method": method})

    def wait(self, request_id, timeout=60):
        deadline = time.time() + timeout
        with self.cond:
            while request_id not in self.replies:
                left = deadline - time.time()
                if left <= 0:
                    return None
                self.cond.wait(left)
            return self.replies.pop(request_id)

    def ask(self, method, params):
        return self.wait(self.request(method, params))


MCP = None
TOOLS = None
record["bridge_env"] = [k for k in ("MIYU_ANTIGRAVITY_BRIDGE", "MIYU_ANTIGRAVITY_TOKEN") if k in os.environ]
try:
    with open(os.path.join(HOME, ".gemini", "config", "mcp_config.json")) as file:
        MIYU = json.load(file).get("mcpServers", {}).get("miyu")
except (OSError, ValueError):
    MIYU = None
if MIYU:
    MCP = Mcp(MIYU["command"], MIYU.get("args", []))
    record["mcp_pid"] = MCP.proc.pid
    # 真 agy 1.4.0 的顺序：server/discover（不认识）、roots 通知、initialize、initialized 通知、tools/list。
    MCP.ask("server/discover", {})
    MCP.notify("notifications/roots/list_changed")
    MCP.ask("initialize", {"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "agy"}})
    MCP.notify("notifications/initialized")
    listed = MCP.ask("tools/list", {})
    TOOLS = listed["result"]
    with open(os.path.join(RECORD, "%d.tools" % os.getpid()), "w") as file:
        json.dump(TOOLS, file)
    record["calls"] = []
    record["replies"] = []

PLANS = fake_agy_plans.PLANS


FIRST = True


def init(agent=AGENT):
    if not FIRST:
        return
    out({"event": "init", "conversation_id": CONVERSATION, "init": {"model": option("--model"), "agent": agent}})


def step(index, state, kind, **fields):
    fields.update(conversation_id=CONVERSATION, step_index=index, state=state, step_type=kind)
    out({"event": "step_update", "step_update": fields})


def result(status="SUCCESS", **fields):
    fields.update(conversation_id=CONVERSATION, status=status)
    out({"event": "result", "result": fields})


def replay(name, junk=False):
    with open(os.path.join(SAMPLES, name)) as file:
        for raw in file:
            event = json.loads(raw)
            for key in ("init", "step_update", "result"):
                if key in event and "conversation_id" in event[key]:
                    event[key]["conversation_id"] = CONVERSATION
            if "conversation_id" in event:
                event["conversation_id"] = CONVERSATION
            if event["event"] == "init":
                if not FIRST:
                    continue
                event["init"]["agent"] = AGENT
            out(event)
            if junk and event["event"] == "init":
                sys.stdout.write("this line is not JSON\n")
                sys.stdout.flush()


def hold():
    while True:
        time.sleep(1)


SECOND_MODES = ("fail-second", "hang-second", "die-second", "quit-second")


def echo():
    init()
    step(0, "DONE", "user_input")
    time.sleep(0.3)
    step(1, "ACTIVE", "agent_response", text_delta="echo: ")
    time.sleep(0.3)
    step(1, "DONE", "agent_response", text_delta=TEXT[-12:], usage=USAGE)
    result(response="echo: " + TEXT[-12:])



def life_number():
    """第几个起的假 agy（从 0 起）：记录目录里抢一个 life-N 文件。"""
    number = 0
    while True:
        try:
            os.close(os.open(os.path.join(RECORD, "life-%d" % number), os.O_CREAT | os.O_EXCL | os.O_WRONLY))
            return number
        except FileExistsError:
            number += 1


STEP = 0
LAST_CALL = None


def next_step():
    global STEP
    STEP += 1
    return STEP


def call_step(name, state, **info):
    index = LAST_CALL["index"]
    step(index, state, "tool", tool_name="mcp_miyu_" + name, tool_info=info)


def call_tool(name, arguments, active, wait):
    """工具调用：active 是发不发 ACTIVE，wait 是等不等回话（等的发 DONE）。"""
    global LAST_CALL
    LAST_CALL = {"index": next_step(), "name": name}
    if active == "before":
        call_step(name, "ACTIVE", parameters=arguments)
    request_id = MCP.request("tools/call", {"name": name, "arguments": arguments})
    record["calls"].append({"name": name, "arguments": arguments})
    save()
    if active == "after":
        time.sleep(0.2)
        call_step(name, "ACTIVE", parameters=arguments)
    if not wait:
        return
    reply = MCP.wait(request_id)
    record["replies"].append(reply["result"] if reply and "result" in reply else reply)
    save()
    if active != "never":
        text = "".join(block.get("text", "") for block in (reply or {}).get("result", {}).get("content", []))
        call_step(name, "DONE", output=text)


def write_transcript(index, content):
    """照真样本往 agy 的转录里添一行：同一个 step_index、content 是两行时间加字。"""
    path = os.path.join(base, "brain", CONVERSATION, ".system_generated", "logs", "transcript_full.jsonl")
    row = {
        "step_index": index,
        "source": "MODEL",
        "type": "GENERIC",
        "status": "DONE",
        "created_at": "2026-11-14T12:19:37Z",
        "content": "Created At: 2026-11-14T21:19:37+09:00\nCompleted At: 2026-11-14T21:19:43+09:00\n" + content,
    }
    with open(path, "a") as file:
        file.write(json.dumps(row, ensure_ascii=False) + "\n")


def later(delay, function):
    """delay 秒以后在另一个线程里做 function；0 就现在做。"""
    if delay > 0:
        threading.Timer(delay, function).start()
    else:
        function()


def native(name, parameters, how, payload, delay):
    """agy 自己跑的原生工具一步。"""
    index = next_step()
    info = {"name": name, "parameters": parameters}

    def tool(state, **extra):
        step(index, state, "tool", tool_name=name, tool_info=dict(info, **extra))

    if how != "done-only":
        tool("ACTIVE")
    if how == "error":
        tool("ERROR", error={"type": "tool_error", "message": payload})
    elif how == "output":
        tool("DONE", output=payload)
        write_transcript(index, "TRANSCRIPT-LOSES")
    else:
        tool("DONE")
        if how != "silent":
            later(delay, lambda: write_transcript(index, payload))


def run_plan(turns):
    init()
    step(0, "DONE", "user_input")
    for item in turns[min(TURN, len(turns)) - 1]:
        kind = item[0]
        if kind == "text":
            index = next_step()
            step(index, "ACTIVE", "agent_response", text_delta=item[1])
            step(index, "DONE", "agent_response", usage=USAGE)
        elif kind == "chunks":
            index = next_step()
            for chunk in item[1]:
                step(index, "ACTIVE", "agent_response", text_delta=chunk)
            step(index, "DONE", "agent_response", usage=USAGE)
        elif kind == "native":
            native(item[1], item[2], item[3], item[4], item[5] if len(item) > 5 else 0)
        elif kind == "echo":
            index = next_step()
            step(index, "ACTIVE", "agent_response", text_delta="echo: ")
            step(index, "DONE", "agent_response", text_delta=TEXT[-12:], usage=USAGE)
        elif kind == "call":
            call_tool(item[1], item[2], "before", True)
        elif kind == "call-mcp-first":
            call_tool(item[1], item[2], "after", True)
        elif kind == "callnw":
            call_tool(item[1], item[2], "before", False)
        elif kind == "active":
            index = next_step()
            step(index, "ACTIVE", "tool", tool_name="mcp_miyu_" + item[1], tool_info={"parameters": item[2]})
        elif kind == "mcp-only":
            call_tool(item[1], item[2], "never", True)
        elif kind == "speak":
            speak = os.path.join(RECORD, "speak")
            deadline = time.time() + 60
            while not os.path.exists(speak) and time.time() < deadline:
                time.sleep(0.02)
            call_step(
                LAST_CALL["name"],
                "ERROR",
                error={"type": "timeout", "message": 'MCP tool call to server "miyu" timed out after 3m0s'},
            )
            index = next_step()
            step(index, "ACTIVE", "agent_response", text_delta="I gave up waiting.")
            step(index, "DONE", "agent_response", usage=USAGE)
            result()
            for _ in range(10):
                sys.stdout.write("padding " * 12500 + "\n")
                sys.stdout.flush()
            record["spoke"] = True
            save()
            hold()
        elif kind == "hold":
            hold()
    result()


PLAN = None
if MODE.startswith("plan:"):
    lives = PLANS[MODE[len("plan:"):]]
    PLAN = lives[min(life_number(), len(lives) - 1)]
record["lines"] = []
TURN = 0
while True:
    line = sys.stdin.readline()
    if not line:
        sys.exit(0)  # 标准输入关了：照真 agy 退出。
    TURN += 1
    record["lines"].append(line)
    record.setdefault("stdin", line)
    record["turns"] = TURN
    TEXT = json.loads(line)["message"]["content"][-1]["text"]
    save()
    if PLAN is not None:
        run_plan(PLAN)
    elif MODE.startswith("sample:"):
        replay(MODE[len("sample:"):])
    elif MODE == "junk":
        replay("real-pong.jsonl", junk=True)
    elif MODE == "echo" or TURN == 1 and MODE in SECOND_MODES:
        echo()
    elif MODE == "idle-exit":
        echo()
        time.sleep(0.3)
        record["exited"] = True
        save()
        sys.exit(0)
    elif MODE == "fail-second":
        step(1, "DONE", "error_message", text_delta="step failed")
        result("ERROR", response="", error="second turn broke")
    elif MODE == "hang-second":
        child = subprocess.Popen(["sleep", "1000"])
        record["child"] = child.pid
        save()
        hold()
    elif MODE == "die-second":
        step(1, "ACTIVE", "agent_response", text_delta="partial")
        sys.exit(1)
    elif MODE == "quit-second":
        sys.exit(0)
    elif MODE == "long":
        init()
        step(1, "ACTIVE", "agent_response", text_delta="x" * 3000000)
        step(1, "DONE", "agent_response", usage=USAGE)
        result()
    elif MODE == "hang":
        init()
        child = subprocess.Popen(["sleep", "1000"])
        record["child"] = child.pid
        save()
        hold()
    elif MODE == "stubborn":
        init()
        record["ready"] = True
        save()
        hold()
    elif MODE.startswith("fail:"):
        init()
        step(1, "DONE", "error_message", text_delta="step failed")
        result("ERROR", response="", error=MODE[len("fail:"):])
    elif MODE == "noresult":
        init()
        sys.exit(1)
    elif MODE == "noresult-after-text":
        init()
        step(1, "ACTIVE", "agent_response", text_delta="partial")
        sys.exit(1)
    elif MODE == "badagent":
        init("someone-else")
        hold()
    else:
        sys.stderr.write("fake agy: unknown mode %s\n" % MODE)
        sys.exit(2)
    FIRST = False
