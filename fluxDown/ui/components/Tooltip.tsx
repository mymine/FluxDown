import type { ComponentChildren, JSX } from "preact";

/** 轻量提示：直接用原生 title，避免在 Shadow DOM / 面板内处理浮层定位。 */
export function Tooltip(props: { text: string; children?: ComponentChildren }): JSX.Element {
  return <span title={props.text}>{props.children}</span>;
}
