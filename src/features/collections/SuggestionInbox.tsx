import { useEffect, useMemo, useRef, useState } from "react";
import { transitionContent } from "@/lib/contentTransition";
import { ActionIcon, Alert, Badge, Button, Checkbox, Collapse, Group, Loader, SegmentedControl, Stack, Text, TextInput, Tooltip } from "@mantine/core";
import { modals } from "@mantine/modals";
import { notifications } from "@mantine/notifications";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useAppNavigate } from "@/app/router";
import { NamedWorkList } from "@/components/NamedWorkList";
import { WorkCover } from "@/components/WorkCover";
import { AssistLauncher } from "@/features/assist/AssistLauncher";
import { useAssist } from "@/features/assist/useAssist";
import { errorMessage, formatNumber } from "@/lib/format";
import { Icons, IconSize } from "@/lib/icons";
import { getProvider } from "@/lib/providers";
import { demoSuggestions, getDemoReader } from "@/mocks/demoData";
import { nameCollectionSuggestion } from "@/services/assistApi";
import { acceptCollectionSuggestion, listCollectionSuggestions, rejectCollectionSuggestion, suggestionNameOverride } from "@/services/collectionApi";
import { getReaderContentPage, isTauriRuntime } from "@/services/dbApi";
import type { CollectionSuggestion, CollectionSuggestionMember, SavedSearchSuggestion, WorkKey } from "@/types/collections";
import "./discovery.css";

const EMPTY: CollectionSuggestion[] = [];
type Track = "sequence" | "theme";
interface ReviewDraft { name: string; order: string[]; excluded: string[] }

function memberKey(member: WorkKey): string {
  return `${member.source}\u001f${member.sourceId}`;
}

function initialDraft(suggestion: CollectionSuggestion): ReviewDraft {
  return { name: suggestion.proposedName, order: suggestion.members.map(memberKey), excluded: suggestion.members.filter((member) => member.selected === false).map(memberKey) };
}

/** 再解析で作品が増えても、確認中の選択と手で直した順序を壊さない。 */
function orderedMembers(suggestion: CollectionSuggestion, draft: ReviewDraft): CollectionSuggestionMember[] {
  const byKey = new Map(suggestion.members.map((member) => [memberKey(member), member]));
  const ordered = draft.order.flatMap((key) => byKey.has(key) ? [byKey.get(key)!] : []);
  const seen = new Set(draft.order);
  return [...ordered, ...suggestion.members.filter((member) => !seen.has(memberKey(member)))];
}

function selectedMembers(suggestion: CollectionSuggestion, draft: ReviewDraft): CollectionSuggestionMember[] {
  return orderedMembers(suggestion, draft).filter((member) => !draft.excluded.includes(memberKey(member)));
}

/** 候補を切り替えても下書きを残す。保留は確認の順番を変えるだけで、否定を保存しない。 */
export function SuggestionInbox({ sweeping, savedSearchIdeas, note, onBusyChange }: {
  sweeping: boolean;
  savedSearchIdeas: SavedSearchSuggestion[];
  note?: string | null;
  onBusyChange?: (busy: boolean) => void;
}) {
  const runtime = isTauriRuntime();
  const navigate = useAppNavigate();
  const queryClient = useQueryClient();
  const { engine } = useAssist("collection_naming");
  const [track, setTrack] = useState<Track>("sequence");
  const [search, setSearch] = useState("");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [drafts, setDrafts] = useState<Record<string, ReviewDraft>>({});
  const [deferred, setDeferred] = useState<ReadonlySet<string>>(new Set());
  const [resolved, setResolved] = useState<ReadonlySet<string>>(new Set());
  const [showDeferred, setShowDeferred] = useState(false);
  const [savedSearchOpen, setSavedSearchOpen] = useState(false);
  const [created, setCreated] = useState<{ id: string; name: string } | null>(null);
  const [mobileDetail, setMobileDetail] = useState(false);
  const candidateButtons = useRef(new Map<string, HTMLButtonElement>());

  const suggestionsQuery = useQuery({
    queryKey: ["collection-suggestions", "pending"],
    queryFn: () => runtime ? listCollectionSuggestions("pending") : Promise.resolve(demoSuggestions),
  });
  const suggestions = (suggestionsQuery.data ?? EMPTY).filter((suggestion) => !resolved.has(suggestion.id)).sort((a, b) =>
    Number(b.members.some((m) => m.evidence.some((e) => e.kind === "unregistered"))) - Number(a.members.some((m) => m.evidence.some((e) => e.kind === "unregistered"))));
  const counts = { sequence: suggestions.filter((suggestion) => suggestion.track !== "theme").length, theme: suggestions.filter((suggestion) => suggestion.track === "theme").length };
  const inTrack = suggestions.filter((suggestion) => (suggestion.track === "theme" ? "theme" : "sequence") === track);
  const deferredCount = inTrack.filter((suggestion) => deferred.has(suggestion.id)).length;
  const term = search.trim().toLocaleLowerCase();
  const filtered = inTrack.filter((suggestion) => deferred.has(suggestion.id) === showDeferred && (!term ||
    [suggestion.proposedName, ...suggestion.members.flatMap((member) => [member.title, member.authorName, getProvider(member.source).label])].some((value) => value.toLocaleLowerCase().includes(term))));
  const selected = filtered.find((suggestion) => suggestion.id === selectedId) ?? filtered[0];
  const selectedIndex = selected ? filtered.findIndex((suggestion) => suggestion.id === selected.id) : -1;
  const draft = selected ? drafts[selected.id] ?? initialDraft(selected) : null;

  const updateDraft = (suggestion: CollectionSuggestion, change: (current: ReviewDraft) => ReviewDraft) => {
    setDrafts((current) => ({ ...current, [suggestion.id]: change(current[suggestion.id] ?? initialDraft(suggestion)) }));
  };
  const invalidate = () => {
    queryClient.invalidateQueries({ queryKey: ["collection-suggestions"] });
    queryClient.invalidateQueries({ queryKey: ["work-collections"] });
  };
  const finish = (id: string) => {
    setResolved((current) => new Set([...current, id]));
    invalidate();
  };
  const accept = useMutation({
    mutationFn: ({ suggestion, draft: review }: { suggestion: CollectionSuggestion; draft: ReviewDraft }) => acceptCollectionSuggestion({
      suggestionId: suggestion.id,
      memberKeys: selectedMembers(suggestion, review).map(({ source, sourceId }) => ({ source, sourceId })),
      name: suggestionNameOverride(suggestion.proposedName, review.name.trim()),
    }),
    onSuccess: (collection, { suggestion, draft: review }) => { finish(suggestion.id); setCreated({ id: collection.id, name: review.name.trim() }); },
    onError: (error) => notifications.show({ color: "red", title: "採用できません", message: errorMessage(error) }),
  });
  const reject = useMutation({
    mutationFn: ({ suggestion, members }: { suggestion: CollectionSuggestion; members: CollectionSuggestionMember[] }) => rejectCollectionSuggestion(suggestion.id, members.map(({ source, sourceId }) => ({ source, sourceId }))),
    onSuccess: (_, { suggestion }) => { finish(suggestion.id); notifications.show({ message: "選んだ組合せを候補から外しました" }); },
    onError: (error) => notifications.show({ color: "red", title: "候補から外せません", message: errorMessage(error) }),
  });
  const askModel = useMutation({
    mutationFn: (suggestion: CollectionSuggestion) => {
      if (!engine) return Promise.reject(new Error("モデルの手伝いが設定されていません"));
      return nameCollectionSuggestion(suggestion.id, engine);
    },
    onSuccess: (updated) => {
      queryClient.setQueryData<CollectionSuggestion[]>(["collection-suggestions", "pending"], (current) => current?.map((item) => item.id === updated.id ? updated : item));
      const proposed = updated.nameOptions.find((option) => option.source === "llm");
      if (proposed) updateDraft(updated, (current) => ({ ...current, name: proposed.name }));
    },
    onError: (error) => notifications.show({ color: "red", title: "名前案を作れません", message: errorMessage(error) }),
  });
  const busy = sweeping || accept.isPending || reject.isPending || askModel.isPending;
  useEffect(() => { onBusyChange?.(accept.isPending || reject.isPending || askModel.isPending); }, [accept.isPending, reject.isPending, askModel.isPending, onBusyChange]);

  const confirmReject = (suggestion: CollectionSuggestion, review: ReviewDraft) => {
    const members = selectedMembers(suggestion, review);
    modals.openConfirmModal({
      title: "この組合せを候補から外す",
      children: <Stack gap="sm"><Text size="sm">選んだ{formatNumber(members.length)}作品の組合せを、同じ規則では再提案しません。チェックを外した作品は対象になりません。</Text><NamedWorkList works={members} /></Stack>,
      labels: { confirm: "候補から外す", cancel: "キャンセル" },
      confirmProps: { color: "red" },
      onConfirm: () => reject.mutate({ suggestion, members }),
    });
  };
  const defer = (suggestion: CollectionSuggestion) => {
    setDeferred((current) => { const next = new Set(current); if (showDeferred) next.delete(suggestion.id); else next.add(suggestion.id); return next; });
  };
  const choose = (id: string) => { setSelectedId(id); setMobileDetail(true); };

  return (
    <div className="discovery-inbox" aria-busy={sweeping || undefined}>
      <div className="discovery-filters">
        <SegmentedControl aria-label="まとまりの種類" value={track} disabled={busy} onChange={(value) => transitionContent(() => { setTrack(value as Track); setShowDeferred(false); setMobileDetail(false); }, { content: () => document.querySelector<HTMLElement>(".discovery-workspace, .discovery-empty") })} data={[{ value: "sequence", label: `続き物 ${formatNumber(counts.sequence)}` }, { value: "theme", label: `テーマ ${formatNumber(counts.theme)}` }]} size="sm" />
        <Text size="xs" c="dimmed">{track === "sequence" ? "シリーズの内側や、保存元をまたぐ続き物も確認できます。" : "読む順を持たない、題材の近い作品のまとまりです。"}</Text>
      </div>
      {sweeping && <div className="discovery-notice" role="status"><Loader size="xs" /><Text size="sm">棚を調べています。確認中の候補はそのまま残ります。</Text></div>}
      {!sweeping && note && <Alert color="yellow" title="一部しか探せていません" icon={<Icons.info size={IconSize.action} />}>{note}</Alert>}
      {created && <div className="discovery-created" role="status"><Icons.confirm size={IconSize.action} /><Text size="sm">「{created.name}」を作りました。</Text><Button variant="subtle" size="compact-xs" onClick={() => navigate(`/collections/${created.id}`)}>開く</Button></div>}
      {suggestionsQuery.isPending ? (
        <div className="discovery-empty" role="status"><Loader size="sm" /><Text c="dimmed">候補を読み込んでいます</Text></div>
      ) : suggestionsQuery.error ? (
        <Alert color="red" title="候補を読み込めません"><Stack gap="sm"><Text size="sm">{errorMessage(suggestionsQuery.error)}</Text><Button variant="light" size="compact-sm" onClick={() => suggestionsQuery.refetch()}>もう一度読み込む</Button></Stack></Alert>
      ) : suggestions.length === 0 ? (
        <div className="discovery-empty"><Icons.collection size={IconSize.hero} strokeWidth={1.3} /><Text fw={650}>{created ? "すべての候補を確認しました" : "読む順のあるまとまりを見つける"}</Text><Text size="sm" c="dimmed">「棚から探す」で、保存した作品から候補を探します。</Text><Text size="xs" c="dimmed">採用するまで、コレクションは作成されません。</Text></div>
      ) : (
        <div className="discovery-workspace" data-detail={mobileDetail || undefined}>
          <aside className="discovery-sidebar" aria-label="まとまりの候補">
            <div className="discovery-sidebar__tools">
              <TextInput aria-label="候補を絞り込む" placeholder="題名・作者で絞り込む" value={search} onChange={(event) => setSearch(event.currentTarget.value)} leftSection={<Icons.search size={IconSize.menu} />} size="sm" disabled={busy} />
              <Group justify="space-between" gap="xs"><Text size="xs" c="dimmed">{showDeferred ? "保留中" : "候補"} {formatNumber(filtered.length)}件</Text><Button variant={showDeferred ? "light" : "subtle"} color="gray" size="compact-xs" disabled={busy} onClick={() => setShowDeferred((value) => !value)}>{showDeferred ? "候補に戻る" : `保留 ${formatNumber(deferredCount)}`}</Button></Group>
            </div>
            <ol className="discovery-candidates">
              {filtered.map((suggestion, index) => {
                const authors = [...new Set(suggestion.members.map((member) => member.authorName))];
                const sources = [...new Set(suggestion.members.map((member) => getProvider(member.source).label))];
                const discoveryScope = suggestion.members.some((m) => m.evidence.some((e) => e.kind === "unregistered")) ? "シリーズ未登録を含む" : suggestion.members.some((m) => m.evidence.some((e) => e.kind === "series_subset")) ? "シリーズ内の続き物" : null;
                return <li key={suggestion.id}>
                  <button
                    ref={(element) => { if (element) candidateButtons.current.set(suggestion.id, element); else candidateButtons.current.delete(suggestion.id); }}
                    type="button" className="discovery-candidate" aria-current={selected?.id === suggestion.id ? "true" : undefined}
                    aria-label={`${suggestion.proposedName}、${suggestion.members.length}作品を確認`} disabled={busy} onClick={() => choose(suggestion.id)}
                    onKeyDown={(event) => {
                      const next = event.key === "ArrowDown" ? index + 1 : event.key === "ArrowUp" ? index - 1 : event.key === "Home" ? 0 : event.key === "End" ? filtered.length - 1 : null;
                      if (next === null || next < 0 || next >= filtered.length) return;
                      event.preventDefault(); setSelectedId(filtered[next].id); candidateButtons.current.get(filtered[next].id)?.focus();
                    }}
                  >
                    <span className="discovery-candidate__number">{String(index + 1).padStart(2, "0")}</span>
                    <span className="discovery-candidate__body"><span className="discovery-candidate__title">{drafts[suggestion.id]?.name || suggestion.proposedName}</span><span className="discovery-candidate__meta">{formatNumber(suggestion.members.length)}作品 · {authors.join("、")}</span><span className="discovery-candidate__source">{sources.join(" / ")}{discoveryScope ? ` · ${discoveryScope}` : ""}</span></span>
                    <Icons.next size={IconSize.menu} className="discovery-candidate__arrow" />
                  </button>
                </li>;
              })}
            </ol>
            {filtered.length === 0 && <div className="discovery-sidebar__empty"><Text size="sm" c="dimmed">{term ? "一致する候補はありません。" : showDeferred ? "保留した候補はありません。" : deferredCount > 0 ? "残りの候補は保留中です。" : track === "sequence" ? "続き物の候補はありません。" : "テーマの候補はありません。"}</Text>{term && <Button variant="subtle" size="compact-sm" onClick={() => setSearch("")}>絞り込みを解除</Button>}</div>}
          </aside>
          <section className="discovery-detail" aria-label="選んだ候補の詳細">
            {selected && draft ? <SuggestionReview key={selected.id} suggestion={selected} draft={draft} busy={busy} accepting={accept.isPending} runtime={runtime} deferred={showDeferred} canNext={selectedIndex + 1 < filtered.length} namingAvailable={Boolean(engine)}
              onDraft={(change) => updateDraft(selected, change)} onAccept={() => accept.mutate({ suggestion: selected, draft })} onDefer={() => defer(selected)} onReject={() => confirmReject(selected, draft)}
              onNext={() => { const next = filtered[selectedIndex + 1]; if (next) setSelectedId(next.id); }} onBack={() => setMobileDetail(false)} onName={() => askModel.mutate(selected)}
            /> : <div className="discovery-empty"><Button className="discovery-mobile-back" variant="subtle" size="compact-sm" leftSection={<Icons.back size={IconSize.menu} />} onClick={() => setMobileDetail(false)}>候補の一覧へ</Button><Icons.collection size={IconSize.hero} strokeWidth={1.3} /><Text size="sm" c="dimmed">{showDeferred ? "保留した候補を、ここで引き続き確認できます。" : "候補を選ぶと、構成作品と読む順を確認できます。"}</Text></div>}
          </section>
        </div>
      )}
      {savedSearchIdeas.length > 0 && <div className="discovery-search-ideas"><Button variant="subtle" color="gray" size="compact-xs" onClick={() => setSavedSearchOpen((value) => !value)} rightSection={<Icons.expand size={IconSize.inline} />}>タグから棚を見る {formatNumber(savedSearchIdeas.length)}</Button><Collapse expanded={savedSearchOpen}><Group gap="xs" pt="xs">{savedSearchIdeas.map((idea) => <Tooltip key={idea.tag} label={idea.reason}><Button variant="default" size="compact-xs" onClick={() => navigate(`/library?tag=${encodeURIComponent(idea.tag)}`)}>{idea.tag} · {formatNumber(idea.workCount)}</Button></Tooltip>)}</Group></Collapse></div>}
    </div>
  );
}

function SuggestionReview({ suggestion, draft, busy, accepting, runtime, deferred, canNext, namingAvailable, onDraft, onAccept, onDefer, onReject, onNext, onBack, onName }: {
  suggestion: CollectionSuggestion; draft: ReviewDraft; busy: boolean; accepting: boolean; runtime: boolean; deferred: boolean; canNext: boolean; namingAvailable: boolean;
  onDraft: (change: (current: ReviewDraft) => ReviewDraft) => void;
  onAccept: () => void; onDefer: () => void; onReject: () => void; onNext: () => void; onBack: () => void; onName: () => void;
}) {
  const [namesOpen, setNamesOpen] = useState(false);
  const [preview, setPreview] = useState<CollectionSuggestionMember | null>(null);
  const [announcement, setAnnouncement] = useState("");
  const headingRef = useRef<HTMLHeadingElement>(null);
  const members = useMemo(() => orderedMembers(suggestion, draft), [suggestion, draft]);
  const chosen = members.filter((member) => !draft.excluded.includes(memberKey(member)));
  const ordered = suggestion.collectionKind !== "unordered";
  const authors = [...new Set(members.map((member) => member.authorName))];
  const sources = [...new Set(members.map((member) => getProvider(member.source).label))];
  const move = (index: number, delta: number) => {
    const next = [...members]; const [member] = next.splice(index, 1); next.splice(index + delta, 0, member);
    onDraft((current) => ({ ...current, order: next.map(memberKey) })); setAnnouncement(`${member.title}を${index + delta + 1}番目へ移動しました`);
  };
  const otherNames = suggestion.nameOptions.filter((option) => option.name !== draft.name);
  const reviewNotes = [...new Set(members.flatMap((member) => member.evidence.filter((item) => ["series_subset", "edition_overlap", "order_conflict", "sequence_gap", "body_unconfirmed"].includes(item.kind)).map((item) => item.label)))];
  if (preview) return <ReaderPreview member={preview} runtime={runtime} onBack={() => { setPreview(null); window.requestAnimationFrame(() => headingRef.current?.focus()); }} />;

  return <>
    <div className="discovery-review__scroll">
      <Button className="discovery-mobile-back" variant="subtle" size="compact-sm" leftSection={<Icons.back size={IconSize.menu} />} onClick={onBack}>候補の一覧へ</Button>
      <header className="discovery-review__heading"><Group gap="xs"><Badge variant="light" color="gray" size="sm">{suggestion.track === "theme" ? "テーマ" : "続き物"}</Badge><Text size="xs" c="dimmed">{formatNumber(members.length)}作品 · {sources.join(" / ")}</Text></Group><h2 ref={headingRef} tabIndex={-1}>{suggestion.proposedName}</h2><Text size="sm" c="dimmed">{authors.join("、")}</Text></header>
      <div className="discovery-evidence"><Icons.link size={IconSize.action} /><div><Text size="xs" fw={650}>まとまりの手がかり</Text><Text size="sm">{suggestion.evidenceSummary || "作品の内容と並びを確認してください。"}</Text></div></div>
      {reviewNotes.length > 0 && <div className="discovery-review-notes"><Text size="xs" fw={650}>確認しておきたいこと</Text><ul>{reviewNotes.map((note) => <li key={note}>{note}</li>)}</ul></div>}
      <section className="discovery-members-section" aria-label="構成作品の確認">
        <Group justify="space-between" align="baseline" gap="xs"><Text component="h3" size="sm" fw={700}>{ordered ? "読む順と構成作品" : "構成作品"}</Text><Text size="xs" c="dimmed">{formatNumber(chosen.length)} / {formatNumber(members.length)}作品を選択</Text></Group>
        <Text size="xs" c="dimmed" mt={4}>{ordered ? "チェックで選び、矢印で読む順を調整できます。" : "コレクションに入れる作品をチェックで選びます。"}</Text>
        <span className="visually-hidden" role="status" aria-live="polite">{announcement}</span>
        <ol className="discovery-members">
          {members.map((member, index) => {
            const key = memberKey(member); const included = !draft.excluded.includes(key);
            const evidence = [...new Set(member.evidence.filter((item) => !["series_subset", "edition_overlap", "order_conflict", "sequence_gap", "body_unconfirmed", "cross_source", "title_similarity", "content_link", "unregistered"].includes(item.kind)).map((item) => item.label).filter(Boolean))];
            return <li key={key} className="discovery-member" data-excluded={!included || undefined}>
              <div className="discovery-member__selection"><Checkbox checked={included} disabled={busy} aria-label={`${member.title}を含める`} onChange={() => onDraft((current) => ({ ...current, excluded: included ? [...current.excluded, key] : current.excluded.filter((value) => value !== key) }))} />{ordered && <span className="discovery-member__position" aria-label={`${index + 1}番目`}>{index + 1}</span>}</div>
              <div className="discovery-member__cover"><WorkCover work={member} variant="compact" /></div>
              <div className="discovery-member__body"><Text size="sm" fw={650} className="discovery-member__title">{member.title}</Text><div className="discovery-member__meta"><span>{getProvider(member.source).label}</span><span>{member.authorName}</span><span>{member.textLength > 0 ? `${formatNumber(member.textLength)}字` : "本文未取得"}</span></div>
                {evidence.length > 0 && <ul className="discovery-member__evidence">{evidence.slice(0, 2).map((label) => <li key={label}>{label}</li>)}</ul>}
                {evidence.length > 2 && <details className="discovery-member__more"><summary>ほかの根拠 {evidence.length - 2}件</summary><ul>{evidence.slice(2).map((label) => <li key={label}>{label}</li>)}</ul></details>}
                <Button variant="subtle" color="gray" size="compact-xs" className="discovery-member__preview" leftSection={<Icons.read size={IconSize.inline} />} disabled={busy || member.downloadId === null} onClick={() => setPreview(member)}>本文を確認</Button>
              </div>
              {ordered && <div className="discovery-member__order"><Tooltip label="一つ前へ"><ActionIcon variant="subtle" color="gray" aria-label={`${member.title}を一つ前へ`} disabled={busy || index === 0} onClick={() => move(index, -1)}><Icons.up size={IconSize.menu} /></ActionIcon></Tooltip><Tooltip label="一つ後へ"><ActionIcon variant="subtle" color="gray" aria-label={`${member.title}を一つ後へ`} disabled={busy || index === members.length - 1} onClick={() => move(index, 1)}><Icons.down size={IconSize.menu} /></ActionIcon></Tooltip></div>}
            </li>;
          })}
        </ol>
      </section>
      <section className="discovery-name" aria-label="コレクションの名前">
        <TextInput label="コレクション名" value={draft.name} onChange={(event) => { const value = event.currentTarget.value; onDraft((current) => ({ ...current, name: value })); }} disabled={busy} />
        <Group justify="space-between" gap="xs" mt={6}>{otherNames.length > 0 && <Button variant="subtle" color="gray" size="compact-xs" rightSection={<Icons.expand size={IconSize.inline} />} onClick={() => setNamesOpen((value) => !value)}>他の名前の案 {formatNumber(otherNames.length)}</Button>}<AssistLauncher size="sm" label="この候補で使える手伝い" items={[{ id: "collection_naming", label: suggestion.nameOptions.some((option) => option.source === "llm") ? "名前案を作り直す" : "名前案を追加する", description: "候補作品の題名とタグから名前を考えます", enabled: namingAvailable && !busy, unavailableReason: namingAvailable ? "ほかの操作が終わるまで待ってください" : "設定でコレクション命名を有効にしてください", onSelect: onName }]} /></Group>
        <Collapse expanded={namesOpen}><div className="discovery-name-options">{otherNames.map((option) => <button type="button" key={`${option.source}-${option.name}`} disabled={busy} onClick={() => onDraft((current) => ({ ...current, name: option.name }))}><span>{option.name}</span><small>{option.label}{option.source === "llm" && option.modelId ? ` · ${option.modelId}` : ""}{option.source === "llm" && option.createdAt ? ` · ${new Date(option.createdAt).toLocaleDateString("ja-JP")}` : ""}</small></button>)}</div></Collapse>
      </section>
      <Button variant="subtle" color="gray" size="compact-xs" className="discovery-reject" disabled={!runtime || busy || chosen.length < 2} onClick={onReject}>この組合せを候補から外す</Button>
    </div>
    <footer className="discovery-review__actions"><div className="discovery-review__secondary"><Button variant="default" size="sm" disabled={busy} onClick={onDefer}>{deferred ? "保留を戻す" : "保留して次へ"}</Button><Tooltip label="保存せずに次の候補を確認"><ActionIcon variant="default" size={36} aria-label="次の候補" disabled={busy || !canNext} onClick={onNext}><Icons.next size={IconSize.nav} /></ActionIcon></Tooltip></div><Tooltip label={!runtime ? "プレビューではコレクションを作成できません" : chosen.length < 2 ? "2作品以上を選んでください" : !draft.name.trim() ? "コレクション名を入力してください" : "この順序でコレクションを作成"}><Button size="sm" leftSection={<Icons.confirm size={IconSize.menu} />} loading={accepting} disabled={!runtime || busy || chosen.length < 2 || !draft.name.trim()} onClick={onAccept}>{formatNumber(chosen.length)}作品で作る</Button></Tooltip></footer>
  </>;
}

function ReaderPreview({ member, runtime, onBack }: { member: CollectionSuggestionMember; runtime: boolean; onBack: () => void }) {
  const [page, setPage] = useState(0);
  const [knownPageCount, setKnownPageCount] = useState(1);
  const contentRef = useRef<HTMLDivElement>(null);
  const content = useQuery({
    queryKey: ["collection-preview", member.downloadId, page],
    queryFn: () => {
      if (runtime) return getReaderContentPage(member.downloadId!, null, page, true);
      const demo = getDemoReader(member.downloadId!);
      return Promise.resolve({ page: 0, pageCount: 1, plainText: demo.plainText, html: demo.html, totalPlainTextChars: demo.plainText.length, sourcePageStarts: [0] });
    },
    enabled: member.downloadId !== null,
    staleTime: 60_000,
  });
  useEffect(() => { if (content.data) setKnownPageCount(content.data.pageCount); }, [content.data]);
  const pageCount = content.data?.pageCount ?? knownPageCount;
  const changePage = (next: number) => { setPage(next); contentRef.current?.scrollTo({ top: 0 }); };
  return <div className="discovery-preview">
    <header className="discovery-preview__heading"><Button variant="subtle" size="compact-sm" leftSection={<Icons.back size={IconSize.menu} />} onClick={onBack}>候補の確認に戻る</Button><Text size="sm" fw={650}>{member.title}</Text><Text size="xs" c="dimmed">{getProvider(member.source).label} · {member.authorName}</Text></header>
    <div ref={contentRef} className="discovery-preview__content" tabIndex={0} aria-label="作品の本文">{content.isPending ? <div className="discovery-empty" role="status"><Loader size="sm" /><Text size="sm" c="dimmed">本文を読み込んでいます</Text></div> : content.error ? <Alert color="red" title="本文を読み込めません"><Text size="sm">{errorMessage(content.error)}</Text><Button variant="subtle" size="compact-sm" onClick={() => content.refetch()}>もう一度読み込む</Button></Alert> : content.data?.plainText.trim() ? <div className="discovery-preview__text">{content.data.plainText}</div> : <div className="discovery-empty"><Text size="sm" c="dimmed">この作品の本文は保存されていません。</Text></div>}</div>
    <footer className="discovery-preview__actions"><Group gap={4}><Button variant="subtle" size="compact-xs" disabled={page === 0 || content.isPending} onClick={() => changePage(0)}>冒頭</Button><Button variant="subtle" size="compact-xs" disabled={page === pageCount - 1 || content.isPending} onClick={() => changePage(pageCount - 1)}>末尾</Button></Group><Group gap="xs"><ActionIcon variant="default" aria-label="本文の前のページ" disabled={page === 0 || content.isPending} onClick={() => changePage(page - 1)}><Icons.previous size={IconSize.menu} /></ActionIcon><Text size="xs" c="dimmed" aria-live="polite">{page + 1} / {pageCount}</Text><ActionIcon variant="default" aria-label="本文の次のページ" disabled={page + 1 >= pageCount || content.isPending} onClick={() => changePage(page + 1)}><Icons.next size={IconSize.menu} /></ActionIcon></Group></footer>
  </div>;
}
