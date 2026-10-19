// Development JSX runtime (`jsxDEV` receives extra source-location args that are ignored).
import type { ViewNode } from "../types/view";
import { Fragment, jsx } from "./jsx-runtime";

export { Fragment };
export type { JSX } from "./jsx-runtime";

export function jsxDEV(
  kind: Parameters<typeof jsx>[0],
  props: Parameters<typeof jsx>[1],
  key?: string | number,
): ViewNode {
  return jsx(kind, props, key);
}
