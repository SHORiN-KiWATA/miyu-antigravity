#!/usr/bin/env python3
"""假的 agy：集成测试用（tests/serve/）。不联网、不碰真的 ~/.gemini：只在它拿到的 HOME 下建会话文件。

用法：fake_agy.py <怎么演> <agy 的参数…>。测试给每种演法写一个 sh 包装，`exec` 到这里（进程号不变）。
环境：FAKE_RECORD 是记录目录（每个进程写一份 <pid>.json：参数、工作目录、HOME、标准输入的第一行 stdin、收到的每一行
lines、子进程号），FAKE_SAMPLES 是真样本的目录。

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
"""

import json
import os
import signal
import subprocess
import sys
import time
import uuid

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
record = {"argv": ARGS, "cwd": os.getcwd(), "home": HOME, "pid": os.getpid(), "conversation": CONVERSATION}


def save():
    path = os.path.join(RECORD, "%d.json" % os.getpid())
    with open(path + ".tmp", "w") as file:
        json.dump(record, file)
    os.rename(path + ".tmp", path)


# agy 记下的这段对话：扩展收尾时要删掉。
base = os.path.join(HOME, ".gemini", "antigravity-cli")
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
    if MODE.startswith("sample:"):
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
