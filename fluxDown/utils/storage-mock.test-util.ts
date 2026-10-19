/**
 * bun 测试用的 `storage.*` 内存实现：只覆盖 get(key | keys | null)、set、remove。
 * 各测试文件在 `mock.module("wxt/browser", …)` 里引用同一份 StorageArea 形状。
 */
export interface MockStorageArea {
  data: Record<string, unknown>;
  get(keys?: string | string[] | null): Promise<Record<string, unknown>>;
  set(values: Record<string, unknown>): Promise<void>;
  remove(keys: string | string[]): Promise<void>;
}

export function createStorageArea(): MockStorageArea {
  const area: MockStorageArea = {
    data: {},
    async get(keys) {
      if (keys === undefined || keys === null) return { ...area.data };
      const list = Array.isArray(keys) ? keys : [keys];
      const out: Record<string, unknown> = {};
      for (const key of list) {
        if (key in area.data) out[key] = structuredClone(area.data[key]);
      }
      return out;
    },
    async set(values) {
      for (const [key, value] of Object.entries(values)) {
        area.data[key] = structuredClone(value);
      }
    },
    async remove(keys) {
      for (const key of Array.isArray(keys) ? keys : [keys]) delete area.data[key];
    },
  };
  return area;
}
