// Extended end-to-end checks for dialog (C5-C8), notes (F1), export/import (F2/F3), Studio (E2/E9) and diagnostics (S2).
// Usage: node extended.mjs <ws-url> <mock-port> <scratch-dir>
import fs from 'node:fs';
import path from 'node:path';

const [wsUrl, mockPort, scratch] = process.argv.slice(2);
const ws = new WebSocket(wsUrl);
const pending = new Map();
const events = [];
let n = 0;
const results = [];
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

ws.onmessage = (m) => {
  const msg = JSON.parse(m.data);
  if (msg.event) { events.push(msg); return; }
  const p = pending.get(msg.id);
  if (p) { pending.delete(msg.id); p(msg); }
};
function call(method, params = {}, timeoutMs = 60000) {
  const id = `x${++n}`;
  return new Promise((resolve) => {
    pending.set(id, resolve);
    ws.send(JSON.stringify({ id, method, params }));
    setTimeout(() => { if (pending.has(id)) { pending.delete(id); resolve({ error: { code: 'TEST_TIMEOUT' } }); } }, timeoutMs);
  });
}
function check(name, ok, detail = '') {
  results.push({ name, ok, detail });
  console.log(`${ok ? 'PASS' : 'FAIL'}  ${name}${detail ? '  — ' + detail : ''}`);
}
async function waitFor(fn, ms) {
  const end = Date.now() + ms;
  while (Date.now() < end) { const v = await fn(); if (v) return v; await sleep(300); }
  return null;
}
const short = (v) => JSON.stringify(v)?.slice(0, 160);

ws.onopen = async () => {
  try {
    await run();
  } catch (e) {
    check('extended script ran without exception', false, String(e));
  }
  console.log(`\nSUMMARY ${results.filter((r) => r.ok).length}/${results.length} passed`);
  ws.close();
  process.exit(results.every((r) => r.ok) ? 0 : 1);
};
ws.onerror = (e) => { console.log('WS error', e.message); process.exit(1); };

async function run() {
  const nid = (await call('notebook.create', { title: '扩展测试' })).result.id;
  const zh = '# 第三章 向量空间\n\n## 3.5 基与维数\n\n向量空间的一组基是线性无关且张成整个空间的向量组。维数定理：同一向量空间的任意两组基包含相同数量的向量，这个数称为维数。\n\n矩阵的行秩与列秩相等，称为秩。';
  const s1 = (await call('source.add_text', { notebook_id: nid, title: '中文讲义.md', text: zh })).result;
  await waitFor(async () => {
    const l = (await call('source.list', { notebook_id: nid })).result?.sources || [];
    return l.length >= 1 && l.every((s) => s.status === 'READY') ? l : null;
  }, 30000);
  await call('profile.upsert', {
    display_name: 'mock', protocol: 'chat_completion', base_url: `http://127.0.0.1:${mockPort}/v1`,
    model_id: 'mock-model', activate: true, context_window: 32000, max_output_tokens: 2000,
  });

  // --- source.preview (B/G)
  const prev = await call('source.preview', { source_id: s1.source.id });
  check('source.preview returns chunks', (prev.result?.chunks || []).length > 0, short(prev.error || prev.result?.chunks?.[0]));

  // --- first turn, then a referring follow-up that must trigger rewrite (C5) and followups (C6)
  const a1 = await call('chat.send', { notebook_id: nid, text: '基与维数是什么关系？' });
  check('chat turn 1 completes', a1.result?.status === 'COMPLETE', a1.error?.message || a1.result?.status);
  const t0 = events.length;
  const a2 = await call('chat.send', { notebook_id: nid, text: '它的定理呢' });
  check('chat turn 2 (referring) completes', a2.result?.status === 'COMPLETE', a2.error?.message || a2.result?.status);
  const steps = events.slice(t0).filter((e) => e.event === 'chat.thought_step').map((e) => JSON.stringify(e.data));
  check('query rewrite step emitted for referring question', steps.some((s) => /改写|rewrite/i.test(s)), steps.join(' | ').slice(0, 200));
  const msgs0 = (await call('chat.history', { notebook_id: nid })).result?.messages || [];
  const finalAssistant = [...msgs0].reverse().find((m) => m.role === 'assistant');
  const fu = await waitFor(async () => events.slice(t0).find((e) => e.event === 'chat.followups' && e.data?.message_id === finalAssistant.id), 8000);
  check('chat.followups event with 3 questions for turn 2', fu?.data?.questions?.length === 3, short(fu?.data));
  const hist = (await call('chat.history', { notebook_id: nid })).result;
  const msgs = hist?.messages || [];
  const lastAssistant = [...msgs].reverse().find((m) => m.role === 'assistant');
  check('followups persisted on message', (lastAssistant?.followups || []).length === 3, short(lastAssistant?.followups));

  // --- citation detail, feedback, regenerate, clear (C8)
  const cit = await call('chat.citation_detail', { message_id: lastAssistant.id, citation_no: 1 });
  check('chat.citation_detail resolves', !!cit.result && !cit.error, short(cit.error || cit.result));
  const fb = await call('chat.set_feedback', { message_id: lastAssistant.id, feedback: 1 });
  check('chat.set_feedback', !!fb.result, short(fb.error));
  const regen = await call('chat.regenerate', { notebook_id: nid });
  check('chat.regenerate completes', regen.result?.status === 'COMPLETE', short(regen.error || regen.result?.status));
  const afterRegen = (await call('chat.history', { notebook_id: nid })).result?.messages || [];
  check('regenerate keeps message count (replaces last answer)', afterRegen.length === msgs.length, `${msgs.length} -> ${afterRegen.length}`);
  const regenId = [...afterRegen].reverse().find((m) => m.role === 'assistant').id;

  // --- notes (F1)
  const ns = await call('note.save', { notebook_id: nid, title: '我的笔记', body_md: '线性无关 => 基' });
  check('note.save', !!ns.result?.id, short(ns.error));
  const nl = (await call('note.list', { notebook_id: nid })).result?.notes || [];
  check('note.list contains saved note', nl.some((x) => x.id === ns.result.id));
  const nm = await call('note.from_message', { message_id: regenId });
  check('note.from_message', !!nm.result?.id, short(nm.error));
  const nts = await call('note.to_source', { note_id: ns.result.id });
  check('note.to_source creates a source', !!nts.result?.id, short(nts.error || nts.result));
  const nd = await call('note.delete', { note_id: nm.result.id });
  check('note.delete', !!nd.result, short(nd.error));

  // --- Studio estimate / create / grade (E9/E2)
  const est = await call('studio.estimate', { notebook_id: nid, artifact_type: 'quiz' });
  check('studio.estimate returns token estimate', !!est.result && !est.error, short(est.error || est.result));
  const q = (await call('studio.create', { notebook_id: nid, artifact_type: 'quiz', config: {} })).result;
  const qa = await waitFor(async () => {
    const a = (await call('studio.get', { artifact_id: q.id })).result;
    return a && a.status === 'READY' ? a : null;
  }, 30000);
  check('quiz READY', !!qa, '');
  if (qa) {
    const g = await call('studio.grade_answer', { artifact_id: q.id, question_id: 'q1', answer: [0] });
    check('studio.grade_answer single choice correct', g.result?.correct === true || g.result?.is_correct === true, short(g.error || g.result));
    const g2 = await call('studio.grade_answer', { artifact_id: q.id, question_id: 'q1', answer: [1] });
    check('studio.grade_answer single choice wrong', g2.result?.correct === false || g2.result?.is_correct === false, short(g2.error || g2.result));
    for (const fmt of ['markdown', 'json']) {
      const out = path.join(scratch, `quiz.${fmt === 'markdown' ? 'md' : 'json'}`);
      const ex = await call('studio.export', { artifact_id: q.id, format: fmt, out_path: out });
      check(`studio.export ${fmt}`, !!ex.result && fs.existsSync(out) && fs.statSync(out).size > 20, short(ex.error));
    }
  }

  // --- export / import (F2/F3)
  const lna = path.join(scratch, 'nb.lna');
  const md = path.join(scratch, 'nb.md');
  const e1 = await call('notebook.export', { notebook_id: nid, format: 'lna', out_path: lna });
  check('notebook.export .lna', !!e1.result && fs.existsSync(lna) && fs.statSync(lna).size > 100, short(e1.error));
  const e2 = await call('notebook.export', { notebook_id: nid, format: 'markdown', out_path: md });
  check('notebook.export markdown', !!e2.result && fs.existsSync(md) && fs.statSync(md).size > 50, short(e2.error));
  const imp = await call('notebook.import', { in_path: lna });
  check('notebook.import .lna returns new notebook id', !!imp.result?.notebook_id && imp.result.notebook_id !== nid, short(imp.error || imp.result));
  if (imp.result?.notebook_id) {
    const l2 = await waitFor(async () => {
      const l = (await call('source.list', { notebook_id: imp.result.notebook_id })).result?.sources || [];
      return l.length >= 1 && l.every((s) => s.status === 'READY') ? l : null;
    }, 30000);
    check('imported notebook sources reach READY', !!l2, '');
    const n2 = (await call('note.list', { notebook_id: imp.result.notebook_id })).result?.notes || [];
    check('imported notebook keeps notes', n2.some((x) => x.title === '我的笔记'), `${n2.length} notes`);
  }
  // path traversal guard (F2)
  const bad = path.join(scratch, 'bad.lna');
  fs.writeFileSync(bad, 'not a zip');
  const impBad = await call('notebook.import', { in_path: bad });
  check('notebook.import rejects a non-zip file', !!impBad.error, short(impBad.error));

  // --- diagnostics (S2)
  const diag = path.join(scratch, 'diag.json');
  const dx = await call('diagnostics.export', { out_path: diag });
  check('diagnostics.export writes file without secrets', !!dx.result && fs.existsSync(diag) && !/sk-[A-Za-z0-9]{8}/.test(fs.readFileSync(diag, 'utf8')), short(dx.error));

  // --- chat.clear (C8)
  const cl = await call('chat.clear', { notebook_id: nid });
  const afterClear = (await call('chat.history', { notebook_id: nid })).result?.messages || [];
  check('chat.clear empties history', !!cl.result && afterClear.length === 0, `${afterClear.length} left`);
}
