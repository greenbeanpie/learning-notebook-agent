"""Minimal OpenAI-compatible streaming mock for smoke tests. Logs every request body."""
import json, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

LOG = sys.argv[2]

QUIZ = {"title": "基与维数测验", "questions": [
    {"id": "q1", "type": "single", "difficulty": "medium", "prompt": "n 维空间中 n+1 个向量必然？",
     "options": ["线性相关", "线性无关", "构成基", "无法确定"], "answer_indices": [0],
     "accepted_answers": [], "rubric": "", "explanation": "维数定理", "citations": [1]}]}
MINDMAP_ROOT = {"title": "线性代数", "nodes": [
    {"id": "n1", "parent_id": None, "label": "基与维数", "citations": [1]},
    {"id": "n2", "parent_id": "n1", "label": "维数定理", "citations": [1]}]}
SUMMARY = {"title_suggestion": "线性代数笔记", "summary_md": "本笔记本讨论基与维数。",
           "suggested_questions": ["什么是基？", "维数如何定义？", "秩是什么？"]}


def pick_reply(body):
    system = ""
    for m in body.get("messages", []):
        if m.get("role") == "system":
            system += str(m.get("content"))
    if '"standalone"' in system:
        return json.dumps({"standalone": "向量空间的基与维数有什么关系", "queries": ["基 维数", "basis dimension"]}, ensure_ascii=False)
    if '"questions"' in system and "追问" in system:
        return json.dumps({"questions": ["什么是线性无关？", "如何求维数？", "秩与维数有何联系？"]}, ensure_ascii=False)
    if '"summary"' in system and "不超过 300 字" in system:
        return json.dumps({"summary": "用户在讨论基与维数。"}, ensure_ascii=False)
    if "quiz" in system:
        return json.dumps(QUIZ, ensure_ascii=False)
    if "mindmap" in system:
        return json.dumps(MINDMAP_ROOT, ensure_ascii=False)
    if "summary_md" in system or "摘要" in system:
        return json.dumps(SUMMARY, ensure_ascii=False)
    if "JSON" in system:
        return json.dumps({"ok": True})
    return "根据所选来源，向量空间任意两组基包含相同数量的向量，这个数就是维数[1]。区间 [1,2] 只是示例。"


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_POST(self):
        n = int(self.headers.get("content-length", 0))
        body = json.loads(self.rfile.read(n) or b"{}")
        with open(LOG, "a", encoding="utf-8") as f:
            f.write(json.dumps({"path": self.path, "body": body}, ensure_ascii=False) + "\n")
        reply = pick_reply(body)
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.end_headers()
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
