// Fake OpenAI-compatible embedding server for e2e tests (no real API is
// ever called). Same idea as tidy-core's MockEmbedder: words and character
// trigrams hashed into buckets, plus synonym groups that share a reserved
// bucket so "meaning" can be tested deterministically.
//
//   node e2e/fake-embed.mjs [port]          (default 18080)
//   POST /v1/embeddings  {model, input:[...]}
//   POST /control        {fail: true|false}   simulate an outage
//   GET  /stats          {calls, texts, fail}

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
    if (req.method === 'GET' && req.url === '/stats') return send(200, { calls, texts, fail });
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
    send(404, { error: { message: 'not found' } });
  });
});

server.listen(PORT, '127.0.0.1', () => console.log(`fake embeddings on http://127.0.0.1:${PORT}/v1`));
