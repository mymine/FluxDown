/** 表格视图:可排序列(持久化到 localStorage)、按 Status 分组折叠。 */
import { Fragment, useMemo, useState } from "react";
import { ChevronDown, ChevronRight, ChevronUp, ChevronsUpDown, MessageSquare } from "lucide-react";
import type { Lang } from "@/i18n/config";
import { feedback } from "@/i18n/messages/feedback";
import { cn } from "@/lib/utils";
import { LabelChip, StateIcon, formatDate, timeAgo } from "../shared";
import { itemState, matchesFilter, matchesSearch, optionColor, type BoardData, type BoardItem, type ViewConfig } from "./model";

type SortDir = "ASC" | "DESC";
type SortField = "number" | "title" | "status" | "createdAt" | "comments";

/** `narrow: false` 的列在窄屏(< sm)隐藏:状态并入标题单元格,避免整张表横向滚出视口。 */
const COLS: {
  key: SortField | "labels";
  label: keyof (typeof feedback)["en"]["board"]["col"];
  sortable: boolean;
  narrow: boolean;
}[] = [
  { key: "number", label: "number", sortable: true, narrow: false },
  { key: "title", label: "title", sortable: true, narrow: true },
  { key: "status", label: "status", sortable: true, narrow: false },
  { key: "labels", label: "labels", sortable: false, narrow: false },
  { key: "createdAt", label: "created", sortable: true, narrow: false },
  { key: "comments", label: "comments", sortable: true, narrow: true },
];

function Row({ item, rowNum, lang, onOpen }: { item: BoardItem; rowNum: number; lang: Lang; onOpen: (n: number) => void }) {
  return (
    <tr className="cm-row" onClick={() => onOpen(item.issueNumber)}>
      <td className="mono num w-14 text-xs text-subtle max-sm:hidden">{rowNum}</td>
      <td className="sm:min-w-64">
        <button
          type="button"
          className="cm-row-title flex items-start gap-2 text-left"
          onClick={(e) => {
            e.stopPropagation();
            onOpen(item.issueNumber);
          }}
        >
          <StateIcon state={itemState(item)} className="mt-0.5" />
          <span className="line-clamp-2 leading-snug">{item.title}</span>
          <span className="mono num shrink-0 pt-px text-xs text-subtle">#{item.issueNumber}</span>
        </button>
        {item.statusName && <span className="cm-status mt-1.5 ml-6 sm:hidden">{item.statusName}</span>}
      </td>
      <td className="whitespace-nowrap max-sm:hidden">
        {item.statusName ? <span className="cm-status">{item.statusName}</span> : <span className="text-subtle">—</span>}
      </td>
      <td className="max-sm:hidden">
        {item.labels.length ? (
          <div className="flex flex-wrap gap-1">
            {item.labels.map((l) => (
              <LabelChip key={l.name} name={l.name} color={l.color} />
            ))}
          </div>
        ) : (
          <span className="text-subtle">—</span>
        )}
      </td>
      <td className="mono whitespace-nowrap text-xs text-subtle max-sm:hidden">
        <time dateTime={item.createdAt} title={formatDate(item.createdAt, lang)}>
          {timeAgo(item.createdAt, lang)}
        </time>
      </td>
      <td className="mono num text-right text-xs whitespace-nowrap text-subtle">
        {item.comments > 0 ? (
          <span className="inline-flex items-center gap-1">
            <MessageSquare aria-hidden className="size-3" />
            {item.comments}
          </span>
        ) : (
          "—"
        )}
      </td>
    </tr>
  );
}

export default function BoardTable({
  view,
  data,
  lang,
  query,
  onOpen,
}: {
  view: ViewConfig;
  data: BoardData;
  lang: Lang;
  query: string;
  onOpen: (n: number) => void;
}) {
  const t = feedback[lang].board;
  const storageKey = `fluxdown-board-sort-${view.id}`;
  const [sort, setSort] = useState<{ field: SortField; dir: SortDir }>(() => {
    try {
      const saved = localStorage.getItem(storageKey);
      if (saved) return JSON.parse(saved) as { field: SortField; dir: SortDir };
    } catch {
      // localStorage 不可用或内容损坏:回退默认排序
    }
    return view.hasGroupBy ? { field: "status", dir: "ASC" } : { field: "number", dir: "DESC" };
  });
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});

  const statusOrder = useMemo(() => new Map(data.columns.map((c, i) => [c.id, i])), [data.columns]);

  const sorted = useMemo(() => {
    const rows = data.allItems.filter((i) => matchesFilter(view.filter, i) && matchesSearch(i, query));
    const cmp = (a: BoardItem, b: BoardItem) => {
      switch (sort.field) {
        case "title":
          return a.title.localeCompare(b.title);
        case "status":
          return (statusOrder.get(a.statusId ?? "") ?? 9999) - (statusOrder.get(b.statusId ?? "") ?? 9999);
        case "createdAt":
          return new Date(a.createdAt).getTime() - new Date(b.createdAt).getTime();
        case "comments":
          return a.comments - b.comments;
        default:
          return a.issueNumber - b.issueNumber;
      }
    };
    return rows.sort((a, b) => (sort.dir === "ASC" ? cmp(a, b) : -cmp(a, b)));
  }, [data.allItems, view.filter, query, sort, statusOrder]);

  const grouping = view.hasGroupBy && sort.field === "status";
  const groups = useMemo(() => {
    if (!grouping) return [];
    const byStatus = new Map<string, BoardItem[]>();
    const none: BoardItem[] = [];
    for (const item of sorted) {
      if (!item.statusId) none.push(item);
      else {
        const bucket = byStatus.get(item.statusId);
        if (bucket) bucket.push(item);
        else byStatus.set(item.statusId, [item]);
      }
    }
    const desc = sort.dir === "DESC";
    const cols = desc ? [...data.columns].reverse() : data.columns;
    const result = cols.flatMap((c) => {
      const items = byStatus.get(c.id);
      return items?.length ? [{ id: c.id, name: c.name, color: c.color, items }] : [];
    });
    if (none.length) {
      const group = { id: "__none__", name: t.noStatus, color: "GRAY", items: none };
      if (desc) result.unshift(group);
      else result.push(group);
    }
    return result;
  }, [grouping, sorted, data.columns, sort.dir, t.noStatus]);

  const toggleSort = (field: SortField) => {
    const next = { field, dir: (sort.field === field && sort.dir === "ASC" ? "DESC" : "ASC") as SortDir };
    setSort(next);
    try {
      localStorage.setItem(storageKey, JSON.stringify(next));
    } catch {
      // localStorage 不可用:仅本次会话生效
    }
  };

  let rowNum = 0;
  return (
    <div className="overflow-x-auto">
      <table className="cm-table">
        <thead>
          <tr>
            {COLS.map((col) => {
              const label = t.col[col.label];
              const active = sort.field === col.key;
              const SortGlyph = !active ? ChevronsUpDown : sort.dir === "ASC" ? ChevronUp : ChevronDown;
              return (
                <th
                  key={col.key}
                  scope="col"
                  className={cn(col.key === "comments" && "text-right", !col.narrow && "max-sm:hidden")}
                  aria-sort={active ? (sort.dir === "ASC" ? "ascending" : "descending") : undefined}
                >
                  {col.sortable ? (
                    <button
                      type="button"
                      className={cn("cm-sort", active && "is-active", col.key === "comments" && "ml-auto")}
                      onClick={() => toggleSort(col.key as SortField)}
                      aria-label={t.sortBy(label)}
                    >
                      {label}
                      <SortGlyph aria-hidden className="size-3" />
                    </button>
                  ) : (
                    label
                  )}
                </th>
              );
            })}
          </tr>
        </thead>
        <tbody>
          {sorted.length === 0 ? (
            <tr>
              <td colSpan={COLS.length} className="py-12 text-center text-sm text-muted">
                {t.empty}
              </td>
            </tr>
          ) : grouping ? (
            groups.map((g) => {
              const isCollapsed = !!collapsed[g.id];
              return (
                <Fragment key={g.id}>
                  <tr className="cm-group">
                    <td colSpan={COLS.length}>
                      <button
                        type="button"
                        className="flex w-full items-center gap-2 text-left"
                        aria-expanded={!isCollapsed}
                        aria-label={t.toggleGroup(g.name)}
                        onClick={() => setCollapsed((p) => ({ ...p, [g.id]: !p[g.id] }))}
                      >
                        <ChevronRight
                          aria-hidden
                          className={cn("size-3.5 text-subtle transition-transform", !isCollapsed && "rotate-90")}
                        />
                        <span className="size-2 rounded-full" style={{ background: optionColor(g.color) }} />
                        <span className="text-sm font-medium text-fg">{g.name}</span>
                        <span className="mono num text-xs text-subtle">{g.items.length}</span>
                      </button>
                    </td>
                  </tr>
                  {!isCollapsed &&
                    g.items.map((item) => {
                      rowNum += 1;
                      return <Row key={item.id} item={item} rowNum={rowNum} lang={lang} onOpen={onOpen} />;
                    })}
                </Fragment>
              );
            })
          ) : (
            sorted.map((item, i) => <Row key={item.id} item={item} rowNum={i + 1} lang={lang} onOpen={onOpen} />)
          )}
        </tbody>
      </table>
    </div>
  );
}
