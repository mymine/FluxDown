// Automatic JSX runtime. Produces the same `{k, p, c}` shape as `flux.ui.h`.
// Configure with `"jsx": "react-jsx", "jsxImportSource": "fluxdown-plugin-sdk/ui"`.
import type { ViewChild, ViewIntrinsicElements, ViewNode } from "../types/view";

/** Node kind of a `<>…</>` result; the parent splices its children in and it never reaches the wire. */
const FRAGMENT_KIND = "#fragment";

/** `<>…</>` — children are spliced into the parent. Not valid as a view's root. */
export function Fragment(props: { children?: ViewChild }): ViewNode {
  return { k: FRAGMENT_KIND, p: {}, c: collect(props.children) };
}

/** Flattens nested arrays and fragments; drops `null` / `false` / `undefined`. */
function collect(children: ViewChild): ViewNode[] {
  const out: ViewNode[] = [];
  const walk = (child: ViewChild): void => {
    if (Array.isArray(child)) {
      for (const inner of child) walk(inner);
    } else if (child !== null && child !== undefined && child !== false) {
      if (child.k === FRAGMENT_KIND) out.push(...child.c);
      else out.push(child);
    }
  };
  walk(children);
  return out;
}

type Props = Record<string, unknown>;
type Component = (props: Props) => ViewNode;

/** Builds a view node; children come from `props.children`; `key` lands in `p.key`. */
export function jsx(kind: string | Component, props: Props | null, key?: string | number): ViewNode {
  const { children, ...rest } = props ?? {};
  if (typeof kind === "function") {
    return kind({ ...rest, ...(key !== undefined ? { key } : {}), children });
  }
  const p: Props = {};
  for (const [name, value] of Object.entries(rest)) {
    if (value !== undefined) p[name] = value;
  }
  if (key !== undefined) p.key = key;
  return { k: kind, p, c: collect(children as ViewChild) };
}

export const jsxs = jsx;

export namespace JSX {
  export type Element = ViewNode;
  export interface IntrinsicElements extends ViewIntrinsicElements {}
  export interface ElementChildrenAttribute {
    children: unknown;
  }
  export interface IntrinsicAttributes {
    key?: string | number;
  }
}
