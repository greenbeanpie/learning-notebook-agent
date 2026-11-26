"""Minimal OpenAI-compatible streaming mock for smoke tests. Logs every request body."""
import json, re, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOG = sys.argv[2]

QUIZ = {"title": "基与维数测验", "questions": [
    {"id": "q1", "type": "single", "difficulty": "medium", "prompt": "n 维空间中 n+1 个向量必然？",
     "options": ["线性相关", "线性无关", "构成基", "无法确定"], "answer_indices": [0],
     "accepted_answers": [], "rubric": "", "explanation": "维数定理", "citations": [1]}]}
# The mindmap generator runs in passes: PASS 1 builds root + top-level branches, then one
# APPEND PASS per branch adds finer nodes under it (see generate_mindmap_multipass).
MINDMAP_SKELETON = {"title": "线性代数", "nodes": [
    {"id": "r", "parent_id": None, "label": "线性代数", "summary": "基与维数"},
    {"id": "b1", "parent_id": "r", "label": "基", "summary": "线性无关且张成"},
    {"id": "b2", "parent_id": "r", "label": "维数", "summary": "基中向量个数"},
    {"id": "b3", "parent_id": "r", "label": "秩", "summary": "行秩等于列秩"}]}
STUDIO_EXTRA = {
    "flashcards": {"title": "闪卡", "cards": [
        {"id": "c1", "front": "什么是基？", "back": "线性无关且张成整个空间的向量组", "hint": "两个条件", "citations": [1]},
        {"id": "c2", "front": "什么是维数？", "back": "基中向量的个数", "hint": "", "citations": [1]}]},
    "report": {"title": "基与维数报告", "format": "document", "sections": [
        {"heading": "概述", "body_md": "向量空间的**基**是线性无关的生成组[1]。\n\n- 维数等于基的大小\n- 公式：$\\dim V = n$", "key_takeaways": ["基不唯一", "维数唯一"]}], "citations": [1]},
    "slides": {"title": "基与维数", "slides": [
        {"layout": "title", "title": "基与维数", "bullets": [], "speaker_notes": "开场", "citations": []},
        {"layout": "bullets", "title": "什么是基", "bullets": ["线性无关", "张成整个空间"], "speaker_notes": "讲解定义", "citations": [1]}]},
    "infographic": {"title": "基与维数", "subtitle": "一图读懂", "orientation": "landscape", "style": "clean", "blocks": [
        {"heading": "维数", "metric": "n", "summary": "基中向量的个数", "citations": [1]},
        {"heading": "基", "metric": "2 个条件", "summary": "线性无关且张成", "citations": [1]}]},
    "audio": {"title": "基与维数播客", "speakers": [{"id": "A", "role": "主讲"}, {"id": "B", "role": "嘉宾"}], "turns": [
        {"speaker": "A", "text": "欢迎收听线性代数播客，今天我们讨论基与维数。", "citations": [1]},
        {"speaker": "B", "text": "Welcome! A basis is a linearly independent spanning set.", "citations": [1]}]},
}
SUMMARY = {"title_suggestion": "线性代数笔记", "summary_md": "本笔记本讨论基与维数。",
           "suggested_questions": ["什么是基？", "维数如何定义？", "秩是什么？"]}


def mindmap_reply(body, system):
    """Answers the multi-pass mindmap prompts: skeleton first, then per-branch append passes."""
    if "THIS IS PASS 1" in system:
        return json.dumps(MINDMAP_SKELETON, ensure_ascii=False)
    users = "".join(str(m.get("content")) for m in body.get("messages", []) if m.get("role") == "user")
    m = re.search(r"Node: \[([^\]]+)\]", users)
    anchor = m.group(1) if m else "b1"
    return json.dumps({"nodes": [
        {"id": f"{anchor}_t1", "parent_id": anchor, "label": "定义", "summary": "基本定义"},
        {"id": f"{anchor}_t2", "parent_id": anchor, "label": "性质", "summary": "主要性质"},
        {"id": f"{anchor}_t3", "parent_id": anchor, "label": "例子", "summary": "典型例子"}]}, ensure_ascii=False)


def pick_reply(body):
    system = ""
    for m in body.get("messages", []):
        if m.get("role") == "system":
            system += str(m.get("content"))
    # Must come before the "quiz" check: the mindmap prompts mention "quiz question".
    if "Studio generator producing structured JSON for mindmap" in system:
        return mindmap_reply(body, system)
    if '"standalone"' in system:
        return json.dumps({"standalone": "向量空间的基与维数有什么关系", "queries": ["基 维数", "basis dimension"]}, ensure_ascii=False)
    if '"questions"' in system and ("follow-up" in system or "追问" in system):
        return json.dumps({"questions": ["什么是线性无关？", "如何求维数？", "秩与维数有何联系？"]}, ensure_ascii=False)
    if '"summary"' in system and ("under 300 characters" in system or "不超过 300 字" in system):
        return json.dumps({"summary": "用户在讨论基与维数。"}, ensure_ascii=False)
    if "Local Notebook Studio generator" in system or "Studio 生成器" in system:
        for kind, payload in STUDIO_EXTRA.items():
            if f"for {kind}," in system or f"生成 {kind} " in system:
                return json.dumps(payload, ensure_ascii=False)
    if "quiz" in system and not body.get("tools"):
        return json.dumps(QUIZ, ensure_ascii=False)
    if "summary_md" in system or "摘要" in system:
        return json.dumps(SUMMARY, ensure_ascii=False)
    if "JSON" in system:
        return json.dumps({"ok": True})
    last_user = ""
    for m in body.get("messages", []):
        if m.get("role") == "user":
            last_user = str(m.get("content"))
    if "公式" in last_user:
        return r"质能方程 $E=mc^2$，以及积分 $$\int_0^1 x\,dx=\frac{1}{2}$$ 来自来源[1]。"
    return "根据所选来源，向量空间任意两组基包含相同数量的向量，这个数就是维数[1]。区间 [1,2] 只是示例。"


def system_of(body):
    return "".join(str(m.get("content")) for m in body.get("messages", []) if m.get("role") == "system")


def is_tool_probe(body):
    return any((t.get("function") or {}).get("name") == "lna_capability_probe" for t in body.get("tools") or [])


def is_probe(body):
    """profile.upsert probes the model: a ping, one tool call and a JSON object."""
    sys_ = system_of(body)
    return is_tool_probe(body) or sys_.startswith("Reply with OK.") or sys_.startswith("Return only a JSON object with ok set to true.")


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        n = int(self.headers.get("content-length", 0))
        body = json.loads(self.rfile.read(n) or b"{}")
        with open(LOG, "a", encoding="utf-8") as f:
            f.write(json.dumps({"path": self.path, "body": body}, ensure_ascii=False) + "\n")
        probe = is_probe(body)
        # A "/flaky/" base URL passes the capability probe but fails every real request.
        if "/flaky/" in self.path and not probe:
            self.send_response(500)
            self.end_headers()
            self.wfile.write(b'{"error":{"message":"mock outage"}}')
            return
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
        if is_tool_probe(body):
            call = {"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "id": "probe_1", "function": {
                "name": "lna_capability_probe", "arguments": "{\"ok\":true}"}}]}, "finish_reason": None}]}
            end = {"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]}
            for frame in (call, end):
                self.wfile.write(("data: " + json.dumps(frame) + chr(10) + chr(10)).encode())
            self.wfile.write(("data: [DONE]" + chr(10) + chr(10)).encode())
            return
        reply = pick_reply(body)
        parts = [reply[i:i + 7] for i in range(0, len(reply), 7)]
        for p in parts:
            chunk = {"choices": [{"index": 0, "delta": {"content": p}, "finish_reason": None}]}
            self.wfile.write(f"data: {json.dumps(chunk, ensure_ascii=False)}\n\n".encode())
        end = {"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]}
        self.wfile.write(f"data: {json.dumps(end)}\n\n".encode())
        usage = {"choices": [], "usage": {"prompt_tokens": 321, "completion_tokens": 45}}
        self.wfile.write(f"data: {json.dumps(usage)}\n\n".encode())
        self.wfile.write(b"data: [DONE]\n\n")


ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
