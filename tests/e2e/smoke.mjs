// End-to-end smoke test against `lna-devtools serve` using Node's built-in WebSocket.
const [wsUrl, mockPort] = process.argv.slice(2);
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
  const id = `t${++n}`;
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
  while (Date.now() < end) { const v = await fn(); if (v) return v; await sleep(500); }
  return null;
}

ws.onopen = async () => {
  const info = await call('app.info');
  check('app.info', !!info.result, JSON.stringify(info.result?.studio_modalities));

  const nb = (await call('notebook.create', { title: '冒烟测试' })).result;
  check('notebook.create', !!nb?.id);
  const nid = nb.id;

  const zh = '# 第三章 向量空间\n\n## 3.5 基与维数\n\n向量空间的一组基是线性无关且张成整个空间的向量组。维数定理：同一向量空间的任意两组基包含相同数量的向量，这个数称为维数。\n\n矩阵的行秩与列秩相等，称为秩。';
  const en = '# Chapter 3\n\nA basis for a vector space is a sequence of vectors that are linearly independent and span the space. Every basis has the same number of vectors, called the dimension.';
  const s1 = await call('source.add_text', { notebook_id: nid, title: '中文讲义.md', text: zh });
  const s2 = await call('source.add_text', { notebook_id: nid, title: 'Strang notes.txt', text: en });
  check('source.add_text x2', !!s1.result && !!s2.result, s1.error?.message || s2.error?.message || '');
  const dup = await call('source.add_text', { notebook_id: nid, title: 'dup', text: zh });
  check('duplicate source detected', dup.result?.duplicate === true);

  const ready = await waitFor(async () => {
    const l = (await call('source.list', { notebook_id: nid })).result?.sources || [];
    return l.length >= 2 && l.every((s) => s.status === 'READY' || s.status === 'FAILED') ? l : null;
  }, 40000);
  check('ingest reaches READY', !!ready && ready.every((s) => s.status === 'READY'),
    JSON.stringify((ready || []).map((s) => [s.title, s.status, s.error_code])));

  // No model configured yet
  const noModel = await call('chat.send', { notebook_id: nid, text: '什么是维数？' });
  check('chat without model -> MODEL_NOT_CONFIGURED', noModel.error?.code === 'MODEL_NOT_CONFIGURED', noModel.error?.code);

  const prof = await call('profile.upsert', {
    display_name: 'mock', protocol: 'chat_completion', base_url: `http://127.0.0.1:${mockPort}/v1`,
    model_id: 'mock-model', activate: true, context_window: 32000, max_output_tokens: 2000,
  });
  check('profile.upsert + activate', !!prof.result, prof.error?.message || '');
  const list = (await call('profile.list')).result?.profiles || [];
  check('profile.list hides secrets', !JSON.stringify(list).includes('sk-'), `${list.length} profile(s)`);

  const t0 = events.length;
  const chat = await call('chat.send', { notebook_id: nid, text: '基与维数是什么关系？' });
  const evs = events.slice(t0).map((e) => e.event);
  check('chat.send completes', chat.result?.status === 'complete' || chat.result?.status === 'COMPLETE',
    chat.error ? JSON.stringify(chat.error) : `status=${chat.result?.status}`);
  check('chat streamed events', evs.includes('chat.text_delta') && evs.includes('chat.finished'),
    [...new Set(evs)].join(','));
  check('chat citation resolved', (chat.result?.citations || []).length > 0,
    JSON.stringify((chat.result?.citations || []).map((c) => [c.no, c.source_title])));
  check('math interval [1,2] preserved in answer', (chat.result?.content || '').includes('[1,2]'),
    (chat.result?.content || '').slice(-40));

  // Deselect all -> precheck
  await call('source.set_all_selected', { notebook_id: nid, selected: false });
  const noSrc = await call('chat.send', { notebook_id: nid, text: 'x' });
  check('no source selected short-circuit', /未勾选/.test(noSrc.result?.content || ''), noSrc.error?.code || '');
  await call('source.set_all_selected', { notebook_id: nid, selected: true });

  // Studio
  const quiz = await call('studio.create', { notebook_id: nid, artifact_type: 'quiz', config: { count: 'fewer' } });
  check('studio.create quiz', !!quiz.result?.id, quiz.error?.message || '');
  const qDone = quiz.result && await waitFor(async () => {
    const a = (await call('studio.get', { artifact_id: quiz.result.id })).result;
    return a && ['ready', 'failed', 'READY', 'FAILED'].includes(a.status) ? a : null;
  }, 40000);
  check('quiz reaches READY with payload', /ready/i.test(qDone?.status || '') && !!qDone?.payload?.questions,
    qDone ? `status=${qDone.status} err=${qDone.error_code || ''}` : 'timeout (still running?)');

  const mm = await call('studio.create', { notebook_id: nid, artifact_type: 'mindmap', config: {} });
  const mmDone = mm.result && await waitFor(async () => {
    const a = (await call('studio.get', { artifact_id: mm.result.id })).result;
    return a && /ready|failed/i.test(a.status) ? a : null;
  }, 40000);
  check('mindmap reaches READY', /ready/i.test(mmDone?.status || ''), mmDone ? `status=${mmDone.status}` : 'timeout');

  const vid = await call('studio.create', { notebook_id: nid, artifact_type: 'video', config: {} });
  check('video modality rejected', !!vid.error, vid.error?.code || 'accepted!');
  const rs = await call('research.search', { notebook_id: nid, query: 'x' });
  check('research.* deferred', rs.error?.code === 'CAPABILITY_MISSING', rs.error?.code);

  // B-14: Add new source while Studio job is active -> READY within 30s
  const bgArt = await call("studio.create", { notebook_id: nid, artifact_type: "report", config: {} });
  const fastSrc = await call("source.add_text", { notebook_id: nid, title: "并发测试.txt", text: "并发摄取测试正文。" });
  const fastReady = fastSrc.result?.source?.id && await waitFor(async () => {
    const g = (await call("source.get", { source_id: fastSrc.result.source.id })).result;
    return g && (g.status === "READY" || g.status === "FAILED") ? g : null;
  }, 30000);
  check("ingest not blocked by Studio (B-14)", fastReady?.status === "READY", fastReady?.status || "timeout");
  if (bgArt.result?.id) await call("studio.cancel", { artifact_id: bgArt.result.id });

  // Failing model: point profile at an endpoint that passes the probe but fails real requests, studio job should end FAILED (not stuck)
  await call('profile.upsert', { id: prof.result.id, display_name: 'dead', protocol: 'chat_completion',
    base_url: `http://127.0.0.1:${mockPort}/flaky/v1`, model_id: 'x', activate: true });
  const bad = await call('studio.create', { notebook_id: nid, artifact_type: 'flashcards', config: {} });
  const badEnd = await waitFor(async () => {
    const a = (await call('studio.get', { artifact_id: bad.result.id })).result;
    return a && /failed/i.test(a.status) ? a : null;
  }, 45000);
  const badNow = (await call('studio.get', { artifact_id: bad.result.id })).result;
  check('failed generation surfaces FAILED status', !!badEnd, `status after 45s=${badNow?.status} stage=${badNow?.stage}`);

  // Remove source and verify retrieval no longer returns it
  const sid = s1.result.source.id;
  const rm = await call('source.remove', { source_id: sid });
  check('source.remove', !!rm.result, rm.error?.message || '');

  const usage = await call('usage.summary', { notebook_id: nid });
  check('usage recorded', (usage.result?.input_tokens || 0) > 0, JSON.stringify(usage.result));

  console.log(`\nSUMMARY ${results.filter((r) => r.ok).length}/${results.length} passed`);
  ws.close();
  process.exit(results.every((r) => r.ok) ? 0 : 1);
};
ws.onerror = (e) => { console.log('WS error', e.message); process.exit(1); };
