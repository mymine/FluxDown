import { describe, expect, test } from "bun:test";
import { Fragment, jsx, jsxs } from "./jsx-runtime";
import { jsxDEV } from "./jsx-dev-runtime";

describe("jsx runtime", () => {
  test("produces {k, p, c} and moves key into p", () => {
    const node = jsx("text", { text: "hi", tone: undefined }, "a");
    expect(node).toEqual({ k: "text", p: { text: "hi", key: "a" }, c: [] });
  });

  test("flattens children and drops null / false / undefined", () => {
    const child = jsx("divider", {});
    const node = jsxs("stack", { children: [child, null, false, undefined, [child, [child]]] });
    expect(node.c).toHaveLength(3);
    expect(node.p).toEqual({});
  });

  test("fragments are spliced into the parent", () => {
    const a = jsx("divider", {}, "a");
    const b = jsx("divider", {}, "b");
    const node = jsx("stack", { children: [Fragment({ children: [a, b] }), a] });
    expect(node.c.map((n) => n.p.key)).toEqual(["a", "b", "a"]);
  });

  test("function components receive props and key", () => {
    const Row = (props: Record<string, unknown>) => jsx("text", { text: String(props.label), key: props.key });
    expect(jsx(Row, { label: "x" }, 7).p).toEqual({ text: "x", key: 7 });
  });

  test("dev runtime matches the production runtime", () => {
    expect(jsxDEV("text", { text: "hi" }, "k")).toEqual(jsx("text", { text: "hi" }, "k"));
  });
});
