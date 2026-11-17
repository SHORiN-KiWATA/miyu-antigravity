"""假 agy 的剧本表（tests/fake_agy.py 的 PLANS）：只有数据，步子怎么演见 fake_agy.py 开头的说明。"""

PLAN_CALL = ("call", "bash", {"command": "ls -l", "n": 2})
QUERY = {"query": "EditorConfig specification"}
# 真样本里 search_web 步的转录字（去掉两行时间以后）。
FOUND = "The search for \"EditorConfig specification\" returned the following summary:\nA simple file format [1]."
# 拦截文案：旧版 Miyu 测试里录的那一句（Google 的内容策略拦截）。
BLOCK = (
    "The prompt could not be submitted. The prompt contains sensitive words that violate Google's "
    "[Generative AI Prohibited Use policy](https://policies.google.com/terms/generative-ai/use-policy)."
)
# 三字节的字 21846 个共 65538 字节：截到 64 KiB 要退到字边界（65535）。
BIG = "字" * 21846
PLANS = {
    # 一件工具调用走完；之后新的一轮照 echo。
    "one": [[[("text", "Let me look. "), PLAN_CALL, ("text", "Listing received.")], [("echo",)]]],
    # 连着两件。
    "two": [[[PLAN_CALL, ("call", "read", {"path": "a.txt"}), ("text", "both done")]]],
    # 第一个进程调一件工具；重放起的新进程只说一句。
    "replay": [[[("text", "Let me look. "), PLAN_CALL, ("text", "after")]], [[("text", "replayed")]]],
    # 挂着时自己说话：等 speak 文件出现再发 ERROR 加正文。
    "speaks": [[[("text", "Let me look. "), ("callnw", "bash", {"command": "ls"}), ("speak",)]], [[("text", "replayed")]]],
    # 不带工具的请求里调一件全局配置里别的对话登记的工具；之后说一句。
    "call-unlisted": [[[("text", "Let me look. "), ("mcp-only", "x", {"a": 1}), ("text", "done")]]],
    # 只发工具步 ACTIVE，不调。
    "stream-only": [[[("text", "Let me look. "), ("active", "bash", {"command": "ls"}), ("hold",)]]],
    # 只调，不发工具步 ACTIVE。
    "mcp-only": [[[("text", "Let me look. "), ("mcp-only", "bash", {"command": "ls"}), ("text", "done")]]],
    # 先调、后发 ACTIVE。
    "mcp-first": [[[("text", "Let me look. "), ("call-mcp-first", "bash", {"command": "ls"}), ("text", "done")]]],
    # 第一个进程连着两件工具；第二个进程（另一段对话）挂住。
    # 原生工具：搜索（转录里有字）、转录晚写、转录一直没有、流里带结果、出错、只有 DONE。
    "search": [[[("native", "search_web", QUERY, "transcript", FOUND), ("text", "Found it.")]]],
    "search-between": [[[("text", "Let me search. "), ("native", "search_web", QUERY, "transcript", FOUND), ("text", "Found it.")]]],
    "search-late": [[[("native", "search_web", QUERY, "transcript", FOUND, 0.3), ("text", "Found it.")]]],
    "search-never": [[[("native", "search_web", QUERY, "silent", ""), ("text", "Found it.")]]],
    "search-output": [[[("native", "search_web", QUERY, "output", "from the stream"), ("text", "Found it.")]]],
    "search-error": [[[("native", "search_web", QUERY, "error", "search backend failed"), ("text", "Found it.")]]],
    "search-done-only": [[[("native", "search_web", QUERY, "done-only", FOUND), ("text", "Found it.")]]],
    "search-big": [[[("native", "search_web", QUERY, "transcript", BIG), ("text", "Found it.")]]],
    # 第一轮照 echo，第二轮（同一个进程，不发 init）里搜索。
    "search-second": [[[("echo",)], [("native", "search_web", QUERY, "transcript", FOUND), ("text", "Found it.")]]],
    # 先搜索，再调 Miyu 的工具（交回）；带结果来以后接着说。
    "search-then-call": [[[("native", "search_web", QUERY, "transcript", FOUND), PLAN_CALL, ("text", "after")]]],
    # 内容策略拦截：整段正文就是拦截文案（分几条）；前半一样后面不一样；只写了开头一半就说完。
    "policy": [[[("chunks", [BLOCK[:20], BLOCK[20:70], BLOCK[70:]])]]],
    "policy-single": [[[("text", BLOCK)]]],
    "policy-lookalike": [[[("chunks", ["The prompt could not be ", "found in the cache."])]]],
    "policy-half": [[[("chunks", ["The prompt could "])]]],
    "two-then-hold": [
        [[PLAN_CALL, ("call", "read", {"path": "a.txt"}), ("text", "both done")]],
        [[("hold",)]],
    ],
}
