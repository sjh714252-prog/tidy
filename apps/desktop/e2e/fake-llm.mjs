// Slow, long-winded fake LLM for e2e tests (M11, SPEC §14.6): behaves like
// a real model on a big batch — about one second per file and ~300
// characters of Korean reasoning per file — so a 30-file "업무 맥락으로
// 정리" batch takes ~30 s. Speaks both APIs Tidy uses:
//
//   POST /v1/messages          Anthropic Messages (stop_reason)
//   POST /v1/chat/completions  OpenAI-compatible (finish_reason)
//   POST /control              {delayMs?, maxTokens?, fail?}  change behaviour
//   GET  /stats                {chats, lastFiles, truncated, delayMs, maxTokens, lastUser,
//                               images, lastImageType, classifyUsers}
//
// (M16) A request with a picture (Anthropic `image` block / OpenAI
// `image_url` part) under the "사진 내용을 설명" system prompt gets a
// description chosen by the file name in the user text: "IMG_" camera
// names and "영수증" are a receipt, "회의" a whiteboard, anything else a
// desk. Classification prompts are kept in `classifyUsers`.
//
// Answers follow e2e/fake-embed.mjs (customer from the file name or (M12)
// the first-level folder of its path; folder items "[n] 폴더: …" answered
// with kind "folder"), with
// long reasons. maxTokens (env FAKE_MAX_TOKENS, or /control) caps the
// answer like a model's output limit: a longer answer is cut off and
// reported as stop_reason "max_tokens" / finish_reason "length" — Tidy
// must split the batch and retry. Tokens are counted as characters / 2.5.
// No real API is ever called.
//
//   node e2e/fake-llm.mjs [port]     (default 18095)

import http from 'node:http';
import { contextAnswer, curateAnswer } from './fake-embed.mjs';

const PORT = Number(process.argv[2] || process.env.FAKE_LLM_PORT || 18095);
let delayMs = Number(process.env.FAKE_DELAY_MS ?? 1000);
let maxTokens = Number(process.env.FAKE_MAX_TOKENS ?? 0); // 0 = model limit only (request max_tokens)
let fail = false;
let chats = 0;
let truncated = 0;
let lastFiles = 0;
let lastUser = '';
let images = 0;
let lastImageType = '';
const classifyUsers = [];

/** (M16) Text of a message's content (string or parts) and its pictures. */
function splitContent(content) {
  if (typeof content === 'string') return { text: content, pics: [] };
  const parts = Array.isArray(content) ? content : [];
  const text = parts.filter((p) => p.type === 'text').map((p) => p.text).join('\n');
  const pics = parts
    .map((p) =>
      p.type === 'image' ? p.source?.media_type : p.type === 'image_url' ? /^data:([^;]+);/.exec(p.image_url?.url || '')?.[1] : null,
    )
    .filter(Boolean);
  return { text, pics };
}

function visionAnswer(user) {
  if (/IMG_|영수증/.test(user))
    return { caption: '카페에서 받은 종이 영수증', tags: ['영수증', '카페', '경비'], kind: 'receipt' };
  if (/회의/.test(user))
    return { caption: '회의실 화이트보드에 그린 서버 구성도', tags: ['화이트보드', '회의', '구성도'], kind: 'diagram' };
  return { caption: '책상 위의 노트북과 커피잔', tags: ['책상', '노트북'], kind: 'photo' };
}

const FILLER =
  '파일 이름에 고객사 약칭과 이슈를 뜻하는 낱말, 그리고 여덟 자리 날짜가 함께 들어 있어 같은 건의 자료로 보았습니다. ' +
  '본문 앞부분에서도 같은 고객사의 요금·발송 관련 표현이 반복되고, 같은 주간에 만들어진 다른 파일들과 이름 규칙이 같습니다. ' +
  '자료 종류는 확장자와 본문 형식(로그 줄, 표, 안내 문구)으로 정했습니다. 다만 본문이 짧아 확신도는 파일 이름의 단서에 더 기대고 있습니다.';

/** Reason of about 300 Korean characters. */
const longReason = (name) => `'${name}'을(를) 보면, ${FILLER}`.slice(0, 300);

function answerFor(system, user, pics) {
  if (system.includes('사진 내용을 설명') && pics.length) return { body: visionAnswer(user), files: 1 };
  if (system.includes('업무 자료 정리 보조자')) {
    const a = curateAnswer(user);
    for (const f of a.files) f.reason = longReason(f.reason.replace(/^(파일 이름|폴더) '|'(에서 판단|\(.*)$/g, ''));
    return { body: a, files: a.files.length };
  }
  if (system.includes('업무 자료 분석가')) return { body: contextAnswer(user), files: 1 };
  if (system.includes('파일 정리 보조자')) classifyUsers.push(user);
  return {
    body: { dest: '/tmp/tidy-ai-dest', confidence: 0.6, reason: longReason('파일'), new_folder: true, suggested_rename: null },
    files: 1,
  };
}

const tokens = (text) => Math.ceil(text.length / 2.5);

const server = http.createServer((req, res) => {
  let body = '';
  req.on('data', (c) => (body += c));
  req.on('end', async () => {
    const send = (status, obj) => {
      res.writeHead(status, { 'content-type': 'application/json' });
      res.end(JSON.stringify(obj));
    };
    if (req.method === 'GET' && req.url === '/stats')
      return send(200, { chats, lastFiles, truncated, delayMs, maxTokens, fail, lastUser, images, lastImageType, classifyUsers });
    if (req.method === 'POST' && req.url === '/control') {
      const c = JSON.parse(body || '{}');
      if (c.delayMs !== undefined) delayMs = Number(c.delayMs);
      if (c.maxTokens !== undefined) maxTokens = Number(c.maxTokens);
      if (c.fail !== undefined) fail = !!c.fail;
      return send(200, { delayMs, maxTokens, fail });
    }
    const anthropic = req.method === 'POST' && req.url.endsWith('/v1/messages');
    const openai = req.method === 'POST' && req.url.endsWith('/chat/completions');
    if (!anthropic && !openai) return send(404, { error: { message: 'not found' } });
    chats++;
    if (fail) return send(503, { error: { message: 'fake model is down' } });
    const r = JSON.parse(body);
    const system =
      typeof r.system === 'string' ? r.system : (r.messages || []).find((m) => m.role === 'system')?.content || '';
    const { text: user, pics } = splitContent((r.messages || []).find((m) => m.role === 'user')?.content || '');
    if (pics.length) {
      images += pics.length;
      lastImageType = pics[0];
    }
    const { body: answer, files } = answerFor(system, String(user), pics);
    lastFiles = files;
    lastUser = String(user);
    // A slow model: about one second per file.
    await new Promise((ok) => setTimeout(ok, delayMs * files));
    let text = JSON.stringify(answer);
    const limit = Math.min(r.max_tokens || Infinity, maxTokens || Infinity);
    const cut = tokens(text) > limit;
    if (cut) {
      truncated++;
      text = text.slice(0, Math.floor(limit * 2.5));
    }
    const usage = { in: tokens(String(user)) + tokens(String(system)), out: tokens(text) };
    if (anthropic)
      return send(200, {
        id: `msg_${chats}`,
        type: 'message',
        role: 'assistant',
        model: r.model,
        content: [{ type: 'text', text }],
        stop_reason: cut ? 'max_tokens' : 'end_turn',
        usage: { input_tokens: usage.in, output_tokens: usage.out },
      });
    return send(200, {
      model: r.model,
      choices: [{ index: 0, finish_reason: cut ? 'length' : 'stop', message: { role: 'assistant', content: text } }],
      usage: { prompt_tokens: usage.in, completion_tokens: usage.out },
    });
  });
});

server.listen(PORT, '127.0.0.1', () => console.log(`fake slow LLM on http://127.0.0.1:${PORT}/v1`));
