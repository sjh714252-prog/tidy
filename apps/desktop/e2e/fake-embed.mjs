// Fake OpenAI-compatible server for e2e tests (no real API is ever
// called). Embeddings work like tidy-core's MockEmbedder: words and
// character trigrams hashed into buckets, plus synonym groups that share a
// reserved bucket so "meaning" can be tested deterministically. Chat
// completions answer a classify request with one fixed suggestion, a
// work-context request (M10) per file from its name (customer 바다정보/BADA
// or 하늘통신/SKY, issue 과금/장애/전환/대체발송, date YYYYMMDD in the name,
// kind from the extension; 정책 documents by topic; "애매" = unsure), and
// a dictionary request ("바탕화면에서 뽑기") with fixed candidates.
//
//   node e2e/fake-embed.mjs [port]          (default 18080)
//   POST /v1/embeddings        {model, input:[...]}
//   POST /v1/chat/completions  → {"dest": FAKE_DEST, "confidence": 0.6, ...}
//   POST /control              {fail: true|false}   simulate an outage (503)
//   GET  /stats                {calls, texts, chats, fail}

import http from 'node:http';

const PORT = Number(process.argv[2] || process.env.FAKE_EMBED_PORT || 18080);
const DIM = 256;
const SYNONYMS = [
  ['청구서', 'invoice', '요금', '고지서', '납부'],
  ['여행', 'trip', 'travel', '항공권', '숙소'],
  ['계약', 'contract', '계약서', '약정'],
];

let fail = false;
let calls = 0;
let texts = 0;
let chats = 0;
const DEST = process.env.FAKE_DEST || "/tmp/tidy-ai-dest";
let lastUser = '';

/** M10 work-context answer for every "[n] 이름: …" line of the prompt. */
export function curateAnswer(user) {
  const files = [];
  for (const line of user.split('\n')) {
    const m = line.match(/^\[(\d+)\] 이름: (.+?) \| /);
    if (!m) continue;
    const id = Number(m[1]);
    const name = m[2];
    // "OceanInfo" is a spelling the dictionary does not know (learning test).
    const entity = /바다|BADA/i.test(name)
      ? '바다정보'
      : /하늘|SKY/i.test(name)
        ? '하늘통신'
        : /Ocean/i.test(name)
          ? 'OceanInfo'
          : null;
    const issue = /과금/.test(name)
      ? '과금이슈'
      : /장애/.test(name)
        ? '장애'
        : /전환/.test(name)
          ? '번호전환'
          : /대체발송/.test(name)
            ? '대체발송 중복'
            : '기타 이슈';
    const d = name.match(/(20\d\d)(\d\d)(\d\d)/);
    const ext = name.split('.').pop().toLowerCase();
    const doc_kind =
      ext === 'log' ? '로그' : ['png', 'jpg'].includes(ext) ? '캡처' : ['csv', 'xlsx'].includes(ext) ? '데이터' : /회의/.test(name) ? '회의록' : '안내문서';
    const topic = entity ? null : /정책|방침/.test(name) ? `정책/${name.replace(/\.[^.]+$/, '')}` : '기타/메모';
    files.push({
      id,
      entity,
      issue: !entity && /정책|방침/.test(name) ? '정책' : issue,
      date: d ? `${d[1]}-${d[2]}-${d[3]}` : null,
      doc_kind,
      topic,
      confidence: /애매/.test(name) ? 0.3 : 0.95,
      reason: `파일 이름 '${name}'에서 판단`,
    });
  }
  return { files };
}

const CONTEXT_ANSWER = {
  entities: [
    { name: '바다정보', aliases: ['BADA'], files: 3, reason: '파일 이름에 자주 나옴' },
    { name: '하늘통신', aliases: ['SKY'], files: 2, reason: '파일 이름에 자주 나옴' },
  ],
  issue_types: ['과금', '장애', '번호전환'],
  doc_kinds: [{ name: '로그', ext: ['log'] }],
};

function fnv(s) {
  let h = 0xcbf29ce484222325n;
  for (const b of Buffer.from(s, 'utf8')) {
    h ^= BigInt(b);
    h = (h * 0x100000001b3n) & 0xffffffffffffffffn;
  }
  return h;
}

function bucket(feature) {
  const reserved = SYNONYMS.length;
  return reserved + Number(fnv(feature) % BigInt(DIM - reserved));
}

export function vector(text) {
  const v = new Array(DIM).fill(0);
  const words = text.toLowerCase().split(/[^\p{L}\p{N}]+/u).filter(Boolean);
  for (const w of words) {
    const g = SYNONYMS.findIndex((grp) => grp.some((s) => w.includes(s)));
    if (g >= 0) {
      v[g] += 4;
      continue;
    }
    v[bucket(`w:${w}`)] += 1;
    const chars = [...w];
    for (let i = 0; i + 3 <= chars.length; i++) {
      v[bucket(`t:${chars.slice(i, i + 3).join('')}`)] += 0.5;
    }
  }
  return v;
}

const server = http.createServer((req, res) => {
  let body = '';
  req.on('data', (c) => (body += c));
  req.on('end', () => {
    const send = (status, obj) => {
      res.writeHead(status, { 'content-type': 'application/json' });
      res.end(JSON.stringify(obj));
    };
    if (req.method === 'GET' && req.url === '/stats') return send(200, { calls, texts, chats, fail, lastUser });
    if (req.method === 'POST' && req.url === '/control') {
      fail = !!JSON.parse(body || '{}').fail;
      return send(200, { fail });
    }
    if (req.method === 'POST' && req.url.endsWith('/embeddings')) {
      calls++;
      if (fail) return send(503, { error: { message: 'fake embedder is down' } });
      const { input, model } = JSON.parse(body);
      const list = Array.isArray(input) ? input : [input];
      texts += list.length;
      return send(200, {
        object: 'list',
        model,
        data: list.map((t, index) => ({ object: 'embedding', index, embedding: vector(t) })),
        usage: { prompt_tokens: list.join(' ').length, total_tokens: list.join(' ').length },
      });
    }
    if (req.method === 'POST' && req.url.endsWith('/chat/completions')) {
      chats++;
      if (fail) return send(503, { error: { message: 'fake model is down' } });
      const req = JSON.parse(body);
      const sys = (req.messages || []).find((m) => m.role === 'system')?.content || '';
      const user = (req.messages || []).find((m) => m.role === 'user')?.content || '';
      lastUser = user;
      const answer = sys.includes('업무 자료 정리 보조자')
        ? curateAnswer(user)
        : sys.includes('업무 자료 분석가')
          ? CONTEXT_ANSWER
          : { dest: DEST, confidence: 0.6, reason: '테스트용 고정 제안', new_folder: true, suggested_rename: null };
      return send(200, {
        model: JSON.parse(body).model,
        choices: [{ index: 0, finish_reason: 'stop', message: { role: 'assistant', content: JSON.stringify(answer) } }],
        usage: { prompt_tokens: 50, completion_tokens: 20 },
      });
    }
    send(404, { error: { message: 'not found' } });
  });
});

server.listen(PORT, '127.0.0.1', () => console.log(`fake OpenAI API on http://127.0.0.1:${PORT}/v1`));
