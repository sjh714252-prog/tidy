import { useEffect, useMemo, useState } from "react";
import * as ipc from "../../ipc/commands";
import type { AgentSettings, AgentWatch, ComposeResult, NasState, PathMapping, RuleCard } from "../../ipc/types";
import { nasFolderName, useNas } from "../../store/nas";
import { FolderTree } from "../FolderTree";
import { RuleCards } from "../RuleCards";

/**
 * 설정 › NAS (M18, SPEC §14.15): install the Tidy agent on a NAS (compose
 * file with a fresh password), connect, then edit what the NAS watches and
 * its rules. NAS settings are saved to the NAS right away with their own
 * button ("NAS에 저장"), not with the app's save bar.
 */
export function NasSettings() {
  const { list, loaded, load, error } = useNas();
  const [adding, setAdding] = useState(false);
  useEffect(() => {
    void load();
  }, [load]);

  return (
    <>
      <div className="page-head">
        <div>
          <h1>NAS</h1>
          <p className="lead">
            NAS에 Tidy를 설치해 두면 PC가 꺼져 있어도 NAS가 직접 새 파일을 정리하고 검색할 수 있게 해 둡니다. 확인 대기·되돌리기·검색은
            이 PC에서 함께 봅니다.
          </p>
        </div>
        {list.length > 0 && !adding && <button onClick={() => setAdding(true)}>+ NAS 추가</button>}
      </div>
      {!loaded && <p className="muted">불러오는 중…</p>}
      {error && <p className="error">{error}</p>}
      {list.map((n) => (
        <NasPanel key={n.name} n={n} />
      ))}
      {loaded && (list.length === 0 || adding) && (
        <NasSetup existing={list.map((n) => n.name)} onDone={() => setAdding(false)} />
      )}
    </>
  );
}

function NasSetup({ existing, onDone }: { existing: string[]; onDone: () => void }) {
  const [name, setName] = useState(existing.length ? `NAS ${existing.length + 1}` : "NAS");
  const [shares, setShares] = useState("");
  const [volume, setVolume] = useState("/volume1");
  const [compose, setCompose] = useState<ComposeResult | null>(null);
  const [url, setUrl] = useState("");
  const [token, setToken] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const make = async () => {
    if (compose && !window.confirm("새 연결 암호로 다시 만들까요? 이미 NAS에 올렸다면 새 파일로 다시 올려야 합니다.")) return;
    setErr(null);
    try {
      setCompose(await ipc.nasCompose(name, shares, volume));
      setCopied(false);
    } catch (e) {
      setErr(String(e));
    }
  };
  const connect = async () => {
    setBusy(true);
    setErr(null);
    try {
      const n = await ipc.nasConnect(name, url, token.trim() || null);
      useNas.getState().put(n);
      onDone();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };
  const copy = async () => {
    if (!compose) return;
    try {
      await navigator.clipboard.writeText(compose.text);
      setCopied(true);
    } catch {
      setErr("복사하지 못했습니다. 글상자에서 직접 복사하세요.");
    }
  };

  return (
    <section className="card nas-setup" data-testid="nas-setup">
      <div className="card-head">
        <h2>NAS에 Tidy 설치하기</h2>
        <span className="sub">시놀로지·QNAP처럼 Docker(Container Manager)가 되는 NAS</span>
      </div>
      <ol className="nas-steps">
        <li>
          <b>설치 파일 만들기</b>
          <div className="nas-form">
            <label>
              이름
              <input aria-label="NAS 이름" value={name} onChange={(e) => setName(e.target.value)} style={{ width: 120 }} />
            </label>
            <label>
              정리할 공유 폴더
              <input
                aria-label="공유 폴더"
                placeholder="예: 업무, 자료"
                value={shares}
                onChange={(e) => setShares(e.target.value)}
                style={{ width: 200 }}
              />
            </label>
            <label>
              볼륨
              <input aria-label="볼륨" value={volume} onChange={(e) => setVolume(e.target.value)} style={{ width: 100 }} />
            </label>
            <button className="primary" data-testid="nas-make-compose" onClick={() => void make()}>
              설치 파일 만들기
            </button>
          </div>
          {compose && (
            <div className="stack" style={{ gap: 6 }}>
              <p className="ok small" data-testid="nas-compose-note">
                연결 암호를 새로 만들어 이 PC의 키체인에 넣었습니다.
                {compose.saved ? ` 다운로드 폴더에 저장했습니다: ${compose.saved}` : ""}
              </p>
              <textarea className="compose" readOnly value={compose.text} aria-label="docker-compose.yml" data-testid="nas-compose" />
              <div>
                <button onClick={() => void copy()}>{copied ? "복사됨 ✓" : "내용 복사"}</button>
              </div>
            </div>
          )}
        </li>
        <li>
          <b>NAS에 올리기</b>
          <p className="small">
            시놀로지: 패키지 센터에서 <b>Container Manager</b> 설치 → File Station에서 <code>docker/tidy</code> 폴더 만들기 →
            Container Manager › 프로젝트 › 생성 → 경로 <code>docker/tidy</code>, "docker-compose.yml 만들기"에 위 내용 붙여넣기 →
            다음 → 완료. QNAP은 Container Station › 애플리케이션 › 생성에 붙여넣습니다.
          </p>
        </li>
        <li>
          <b>연결</b>
          <div className="nas-form">
            <input
              aria-label="NAS 주소"
              placeholder="NAS 주소 (예: 192.168.0.10)"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              style={{ width: 240 }}
            />
            <button className="primary" data-testid="nas-connect" disabled={busy || !url.trim()} onClick={() => void connect()}>
              {busy ? "확인 중…" : "연결 확인"}
            </button>
          </div>
          <details className="small">
            <summary>설치 파일을 다른 PC에서 만들었거나 직접 설치했다면</summary>
            <div className="nas-form">
              <input
                type="password"
                aria-label="연결 암호"
                placeholder="연결 암호 (TIDY_TOKEN)"
                value={token}
                onChange={(e) => setToken(e.target.value)}
                style={{ width: 280 }}
              />
            </div>
            <span className="muted">
              NAS에서 <code>docker exec tidy-agent tidy serve --print-token</code> 으로 볼 수 있습니다.
            </span>
          </details>
        </li>
      </ol>
      {err && (
        <p className="error" data-testid="nas-setup-error">
          {err}
        </p>
      )}
      {existing.length > 0 && (
        <div>
          <button className="link" onClick={onDone}>
            취소
          </button>
        </div>
      )}
    </section>
  );
}

const emptyWatch = (path: string): AgentWatch => ({
  path,
  alias: null,
  mode: "in_place",
  organizeRoot: null,
  recursive: false,
  exclude: [],
});

/** The shared folder a NAS path is in (`/data/업무/x` → `/data/업무`). */
function shareOf(dataRoot: string, p: string): string | null {
  const root = dataRoot.replace(/\/+$/, "");
  if (!p.startsWith(root + "/")) return null;
  const first = p.slice(root.length + 1).split("/")[0];
  return first ? `${root}/${first}` : null;
}

type Pick = { kind: "watch" } | { kind: "index" } | { kind: "root"; i: number };

function NasPanel({ n }: { n: NasState }) {
  const st = n.status;
  const [saved, setSaved] = useState<AgentSettings | null>(null);
  const [draft, setDraft] = useState<AgentSettings | null>(null);
  const [cards, setCards] = useState<RuleCard[] | null>(null);
  const [rulesError, setRulesError] = useState<string | null>(null);
  const [openRule, setOpenRule] = useState<number | null>(null);
  const [pick, setPick] = useState<Pick | null>(null);
  const [picked, setPicked] = useState<string | null>(null);
  const [maps, setMaps] = useState<PathMapping[]>(n.maps);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);

  const loadSettings = async () => {
    try {
      const s = await ipc.nasSettings(n.name);
      setSaved(s);
      setDraft(s);
      await rulesToCards(s.rules);
    } catch (e) {
      setErr(String(e));
    }
  };
  const rulesToCards = async (text: string) => {
    try {
      const e = await ipc.rulesEditor(text, false);
      setCards(e.cards);
      setRulesError(e.ok ? null : e.error);
    } catch (e) {
      setRulesError(String(e));
    }
  };
  useEffect(() => {
    setMaps(n.maps);
  }, [n.maps]);
  useEffect(() => {
    if (n.ok) void loadSettings();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [n.ok, n.name]);

  const dataRoot = st?.dataRoot ?? saved?.dataRoot ?? "/data";
  const source = useMemo(
    () => ({
      roots: [{ label: `NAS ${dataRoot}`, path: dataRoot, kind: "drive" as const }],
      list: (p: string) => ipc.nasDirs(n.name, p).then((ds) => ds.map((d) => ({ name: d.name, path: d.path }))),
    }),
    [n.name, dataRoot],
  );

  const dirty = !!draft && !!saved && JSON.stringify(draft) !== JSON.stringify(saved);
  const mapsDirty = JSON.stringify(maps) !== JSON.stringify(n.maps);
  const set = (p: Partial<AgentSettings>) => draft && setDraft({ ...draft, ...p });
  const setWatch = (i: number, p: Partial<AgentWatch>) =>
    draft && setDraft({ ...draft, watches: draft.watches.map((w, j) => (j === i ? { ...w, ...p } : w)) });

  const onCards = async (c: RuleCard[]) => {
    if (!draft) return;
    setCards(c);
    try {
      const e = await ipc.rulesFromCards(draft.rules, c, false);
      setDraft({ ...draft, rules: e.text });
      setRulesError(e.ok ? null : e.error);
    } catch (e) {
      setRulesError(String(e));
    }
  };
  const setRulesText = (text: string) => {
    set({ rules: text });
    void rulesToCards(text);
  };
  const copyPcRules = async () => {
    if (!draft) return;
    try {
      const pc = await ipc.getSettings();
      if (draft.rules.trim() && !window.confirm("NAS 규칙을 이 PC의 규칙으로 바꿀까요? (저장 전까지 NAS에는 반영되지 않습니다)")) return;
      setRulesText(pc.rulesText);
    } catch (e) {
      setErr(String(e));
    }
  };

  const confirmPick = () => {
    if (!draft || !pick || !picked) return;
    if (pick.kind === "watch") {
      if (draft.watches.some((w) => w.path === picked)) return setPick(null);
      // Search the whole share (also when it is the watch folder itself):
      // a watch folder covers only its own files, not its sub-folders.
      const share = shareOf(dataRoot, picked);
      const indexOnly =
        share && !draft.indexOnly.includes(share) && !draft.watches.some((w) => w.path === share)
          ? [...draft.indexOnly, share]
          : draft.indexOnly;
      setDraft({ ...draft, watches: [...draft.watches, emptyWatch(picked)], indexOnly });
    } else if (pick.kind === "index") {
      if (!draft.indexOnly.includes(picked)) set({ indexOnly: [...draft.indexOnly, picked] });
    } else {
      setWatch(pick.i, { mode: "collect", organizeRoot: picked });
    }
    setPick(null);
    setPicked(null);
  };

  const save = async () => {
    if (!draft) return;
    setBusy(true);
    setErr(null);
    setMsg(null);
    try {
      const s = await ipc.nasSaveSettings(n.name, draft);
      setSaved(s);
      setDraft(s);
      setMsg("NAS에 저장했습니다. NAS가 감시를 다시 시작하고 폴더를 검색 인덱스에 넣습니다.");
      void useNas.getState().load();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };
  const saveMaps = async () => {
    try {
      await ipc.nasSetMaps(n.name, maps);
      setMsg("PC에서 보이는 경로를 저장했습니다.");
      void useNas.getState().load();
    } catch (e) {
      setErr(String(e));
    }
  };
  const organize = async (path: string) => {
    setBusy(true);
    setErr(null);
    setMsg(null);
    try {
      const p = await ipc.nasOrganizePreview(n.name, path);
      if (!p.toMove) {
        setMsg(`${nasFolderName(path)}: 규칙에 맞는 기존 파일이 없습니다.`);
        return;
      }
      const ex = p.examples
        .slice(0, 3)
        .map(([a, b]) => `${nasFolderName(a)} → ${b.replace(path, "…")}`)
        .join("\n");
      if (!window.confirm(`NAS의 '${nasFolderName(path)}'에 있던 파일 ${p.toMove}개를 규칙대로 옮길까요?\n\n${ex}\n\n되돌리기 탭에서 한 번에 되돌릴 수 있습니다.`))
        return;
      const r = await ipc.nasOrganize(n.name, path);
      setMsg(
        `${r.moved}개를 정리했습니다` + (r.errors.length ? ` · 실패 ${r.errors.length}개: ${r.errors.slice(0, 2).join("; ")}` : ""),
      );
      void useNas.getState().load();
    } catch (e) {
      setErr(String(e));
    } finally {
      setBusy(false);
    }
  };
  const remove = async () => {
    if (!window.confirm(`'${n.name}' 연결을 이 PC에서 지울까요? NAS의 Tidy는 계속 돌고, 다시 연결할 수 있습니다.`)) return;
    try {
      await ipc.nasRemove(n.name);
      useNas.getState().drop(n.name);
    } catch (e) {
      setErr(String(e));
    }
  };

  const anchors = (draft?.watches ?? []).map((w) => (w.mode === "collect" && w.organizeRoot ? w.organizeRoot : w.path));
  const collectRoots = [...new Set((draft?.watches ?? []).filter((w) => w.mode === "collect" && w.organizeRoot).map((w) => w.organizeRoot as string))];

  return (
    <section className="card nas-panel" data-testid="nas-panel" data-name={n.name}>
      <div className="card-head baseline">
        <h2>
          <span className={`dot ${n.ok ? "ok" : "warn"}`} /> {n.name}
        </h2>
        <span className="sub mono">{n.url}</span>
        <div className="spacer" />
        <button className="link" onClick={() => void useNas.getState().load()}>
          새로고침
        </button>
        <button className="link" onClick={() => void remove()}>
          연결 지우기
        </button>
      </div>
      {n.ok && st ? (
        <p className="small" data-testid="nas-status">
          연결됨 · Tidy {st.version} · 감시 폴더 {st.watches.length}개{st.watching ? "" : " (감시 안 함)"} · 확인 대기 {st.pending} ·
          검색 파일 {st.files.toLocaleString()} · 오늘 {st.todayMoved}개 정리
          {st.ocrWaiting > 0 ? ` · 글자 인식 대기 ${st.ocrWaiting}` : ""}
          {st.error && <span className="warn"> · {st.error}</span>}
        </p>
      ) : (
        <p className="warn small" data-testid="nas-status">
          연결 안 됨: {n.error}
        </p>
      )}
      {msg && (
        <div className="notice" data-testid="nas-msg">
          {msg}
        </div>
      )}
      {err && <p className="error">{err}</p>}

      {draft && (
        <>
          <h3>NAS 감시 폴더</h3>
          <p className="muted small">
            여기 새로 생기는 파일을 NAS가 규칙대로 바로 정리합니다. 원래 있던 파일은 [지금 정리]를 눌러야 옮깁니다. 이 폴더를 PC의
            감시 폴더에도 넣어 두었다면 PC 쪽에서는 빼세요(두 곳이 같이 정리하지 않게).
          </p>
          <div className="stack">
            {draft.watches.map((w, i) => {
              const live = st?.watches.find((x) => x.path === w.path);
              return (
                <div className="nas-watch" key={w.path} data-testid="nas-watch" data-path={w.path}>
                  <div className="nas-watch-head">
                    <b>{w.alias || nasFolderName(w.path)}</b>
                    <span className="mono small muted">{w.path}</span>
                    {live && !live.online && <span className="warn small">● 폴더 없음</span>}
                    {live?.polling && <span className="muted small">(주기 확인)</span>}
                    <div className="spacer" />
                    <button
                      disabled={busy || !saved?.watches.some((x) => x.path === w.path)}
                      title={saved?.watches.some((x) => x.path === w.path) ? "" : "먼저 NAS에 저장하세요"}
                      data-testid="nas-organize"
                      onClick={() => void organize(w.path)}
                    >
                      지금 정리
                    </button>
                    <button
                      className="link"
                      onClick={() => draft && setDraft({ ...draft, watches: draft.watches.filter((_, j) => j !== i) })}
                    >
                      빼기
                    </button>
                  </div>
                  <div className="nas-watch-opts small">
                    <label>
                      <input type="checkbox" checked={w.recursive} onChange={(e) => setWatch(i, { recursive: e.target.checked })} />{" "}
                      하위 폴더 안의 파일도 정리
                    </label>
                    <label>
                      정리 위치{" "}
                      <select
                        aria-label="정리 위치"
                        value={w.mode === "collect" && w.organizeRoot ? "collect" : "in_place"}
                        onChange={(e) => {
                          if (e.target.value === "in_place") setWatch(i, { mode: "in_place", organizeRoot: null });
                          else {
                            setPick({ kind: "root", i });
                            setPicked(null);
                          }
                        }}
                      >
                        <option value="in_place">이 폴더 안에서</option>
                        <option value="collect">{w.organizeRoot ? `${nasFolderName(w.organizeRoot)} 아래로 모으기` : "다른 폴더로 모으기…"}</option>
                      </select>
                    </label>
                  </div>
                </div>
              );
            })}
            {!draft.watches.length && <p className="muted small">아직 없습니다. NAS 폴더를 추가하세요.</p>}
          </div>
          <div className="toolbar">
            <button className="dashed" data-testid="nas-add-watch" onClick={() => (setPick({ kind: "watch" }), setPicked(null))}>
              + NAS 폴더 추가
            </button>
          </div>

          <h3>검색만 할 폴더</h3>
          <p className="muted small">옮기지 않고 검색 인덱스에만 넣습니다(공유 폴더 전체를 넣어 두면 NAS 어디에 있든 찾습니다).</p>
          <ul className="nas-index-list">
            {draft.indexOnly.map((p) => (
              <li key={p}>
                <span className="mono small">{p}</span>{" "}
                <button className="link" onClick={() => set({ indexOnly: draft.indexOnly.filter((x) => x !== p) })}>
                  빼기
                </button>
              </li>
            ))}
            <li>
              <button className="link" onClick={() => (setPick({ kind: "index" }), setPicked(null))}>
                + 폴더 추가
              </button>
            </li>
          </ul>

          {pick && (
            <div className="nas-picker" data-testid="nas-picker">
              <FolderTree source={source} selected={picked} onSelect={setPicked} />
              <div className="toolbar">
                <button className="primary" disabled={!picked} data-testid="nas-pick-ok" onClick={confirmPick}>
                  {pick.kind === "root" ? "이 폴더로 모으기" : "이 폴더 추가"}
                </button>
                <button onClick={() => setPick(null)}>취소</button>
              </div>
            </div>
          )}

          <div className="page-head" style={{ marginTop: 8 }}>
            <div>
              <h3>NAS 규칙</h3>
              <p className="muted small">NAS가 쓰는 규칙입니다. 위에서부터 처음 맞는 규칙 하나를 적용하고, 목적지가 상대 경로면 각 감시 폴더(정리 위치) 아래입니다.</p>
            </div>
            <button onClick={() => void copyPcRules()}>PC 규칙 가져오기</button>
            <button onClick={() => void onCards([...(cards ?? []), newCard()]).then(() => setOpenRule((cards ?? []).length))}>
              + 규칙 추가
            </button>
          </div>
          {cards && (
            <RuleCards
              cards={cards}
              preview={null}
              anchors={anchors}
              collectRoots={collectRoots}
              readOnly={!!rulesError && !cards}
              open={openRule}
              onOpen={setOpenRule}
              onChange={(c) => void onCards(c)}
              treeSource={source}
            />
          )}
          {cards && !cards.length && <p className="muted small">규칙이 없으면 새 파일은 모두 확인 대기로 갑니다.</p>}
          {rulesError && <p className="error small">✗ {rulesError}</p>}
          <details className="toml-edit advanced">
            <summary>고급: NAS의 rules.toml 직접 편집</summary>
            <textarea className="rules" aria-label="NAS rules.toml" spellCheck={false} value={draft.rules} onChange={(e) => setRulesText(e.target.value)} />
          </details>

          <div className="toolbar nas-savebar">
            {dirty ? <span className="warn small">NAS에 저장하지 않은 변경이 있습니다</span> : <span className="muted small">NAS 설정은 바로 NAS에 저장됩니다</span>}
            <div className="spacer" />
            <button disabled={!dirty || busy} onClick={() => saved && (setDraft(saved), void rulesToCards(saved.rules))}>
              되돌리기
            </button>
            <button className="primary" data-testid="nas-save" disabled={!dirty || busy || !!rulesError} onClick={() => void save()}>
              {busy ? "저장 중…" : "NAS에 저장"}
            </button>
          </div>
        </>
      )}

      <h3>PC에서 보이는 경로</h3>
      <p className="muted small">
        검색 결과를 이 PC에서 열 때 씁니다. NAS의 폴더가 이 PC에서 어떤 경로(예: <code>\\NAS\업무</code>, <code>Z:\</code>)인지 적어 주세요.
      </p>
      <div className="stack" style={{ gap: 6 }}>
        {maps.map((m, i) => (
          <div className="nas-form" key={i} data-testid="nas-map">
            <input
              aria-label="NAS 경로"
              className="mono"
              value={m.remote}
              onChange={(e) => setMaps(maps.map((x, j) => (j === i ? { ...x, remote: e.target.value } : x)))}
              style={{ width: 200 }}
            />
            →
            <input
              aria-label="PC 경로"
              className="mono"
              value={m.local}
              onChange={(e) => setMaps(maps.map((x, j) => (j === i ? { ...x, local: e.target.value } : x)))}
              style={{ width: 240 }}
            />
            <button className="link" onClick={() => setMaps(maps.filter((_, j) => j !== i))}>
              빼기
            </button>
          </div>
        ))}
        <div className="nas-form">
          <button className="link" onClick={() => setMaps([...maps, { remote: dataRoot + "/", local: "" }])}>
            + 경로 추가
          </button>
          <div className="spacer" />
          <button disabled={!mapsDirty} onClick={() => void saveMaps()}>
            경로 저장
          </button>
        </div>
      </div>
    </section>
  );
}

function newCard(): RuleCard {
  return {
    origin: null,
    name: "새 규칙",
    enabled: true,
    confirm: false,
    ext: [],
    nameContains: "",
    nameRegex: "",
    minSize: "",
    maxSize: "",
    olderThanDays: null,
    newerThanDays: null,
    fromDir: "",
    dest: "",
    encrypt: "",
    otherKeys: [],
  };
}
