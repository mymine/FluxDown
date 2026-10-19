/**
 * 当前站点配置卡：接管（三态）/ 嗅探 / 悬浮球 / 浮钮 / 保存目录。
 * 读写 site-profiles（storage.local），后台经 storage.onChanged 即时生效。
 * collapsible：默认收起为一行摘要（host · 接管模式 · 嗅探关闭提示），点击展开表单；popup 用它省出首屏。
 */
import type { JSX } from "preact";
import { useEffect, useState } from "preact/hooks";
import { browser } from "wxt/browser";
import {
  DEFAULT_SITE_PROFILE,
  loadSiteProfiles,
  profileFor,
  saveSiteProfile,
  SITE_PROFILES_KEY,
  type SiteProfile,
  type SiteProfileFields,
} from "@/utils/site-profiles";
import { Icon } from "@/ui/components/Icon";
import { Select } from "@/ui/components/Select";
import { TextInput } from "@/ui/components/TextInput";
import { Toggle } from "@/ui/components/Toggle";
import { showToast } from "@/ui/components/Toast";
import { useT } from "@/ui/i18n";
import "./features.css";

interface SiteFormProps {
  value: SiteProfileFields;
  onChange: (next: SiteProfileFields) => void;
  /** 保存目录失焦时触发（SiteCard 用来落盘；Modal 表单可忽略）。 */
  onCommitDir?: (next: SiteProfileFields) => void;
}

/** 配置字段表单：SiteCard 与 options `#sites` 的编辑 Modal 共用。 */
export function SiteProfileForm(props: SiteFormProps): JSX.Element {
  const t = useT();
  const { value } = props;
  const set = (patch: Partial<SiteProfileFields>) => props.onChange({ ...value, ...patch });
  return (
    <div class="fd-site-form">
      <div class="fd-site-line">
        <span>{t("site.intercept")}</span>
        <Select
          value={value.intercept}
          onChange={(v) => set({ intercept: v })}
          options={[
            { value: "default", label: t("site.interceptDefault") },
            { value: "never", label: t("site.interceptNever") },
            { value: "always", label: t("site.interceptAlways") },
          ]}
        />
      </div>
      <div class="fd-site-line">
        <span>{t("site.sniff")}</span>
        <Toggle checked={value.sniff} onChange={(v) => set({ sniff: v })} label={t("site.sniff")} />
      </div>
      <div class="fd-site-line">
        <span>{t("site.pageDot")}</span>
        <Toggle checked={value.pageDot} onChange={(v) => set({ pageDot: v })} label={t("site.pageDot")} />
      </div>
      <div class="fd-site-line">
        <span>{t("site.floatButton")}</span>
        <Toggle checked={value.floatButton} onChange={(v) => set({ floatButton: v })} label={t("site.floatButton")} />
      </div>
      <div class="fd-site-dir">
        <span>{t("site.saveDir")}</span>
        <TextInput
          value={value.saveDir}
          placeholder={t("site.saveDirPlaceholder")}
          onInput={(v) => set({ saveDir: v })}
          onCommit={(v) => props.onCommitDir?.({ ...value, saveDir: v })}
        />
        <small class="fd-hint">{t("site.saveDirHint")}</small>
      </div>
    </div>
  );
}

interface SiteCardProps {
  /** 页面 host（TabView.site.host）；空串表示无法获取，卡片不渲染。 */
  host: string;
  collapsible?: boolean;
}

export function SiteCard(props: SiteCardProps): JSX.Element | null {
  const t = useT();
  const { host } = props;
  const [profiles, setProfiles] = useState<Record<string, SiteProfile> | null>(null);
  const [open, setOpen] = useState(!props.collapsible);

  useEffect(() => {
    let alive = true;
    const reload = () => void loadSiteProfiles().then((p) => alive && setProfiles(p));
    reload();
    const onChanged = (changes: Record<string, unknown>, area: string) => {
      if (area === "local" && SITE_PROFILES_KEY in changes) reload();
    };
    browser.storage.onChanged.addListener(onChanged);
    return () => {
      alive = false;
      browser.storage.onChanged.removeListener(onChanged);
    };
  }, []);

  if (!host || !profiles) return null;

  const resolved = profileFor(profiles, host);
  // 无命中时，修改会在当前 host 上新建配置；有命中（可能是父域）时就地更新该 key
  const targetKey = resolved.key ?? host;
  const value = resolved.profile ?? DEFAULT_SITE_PROFILE;

  const save = async (next: SiteProfileFields) => {
    const merged: SiteProfile = { ...next, updatedAt: Date.now() };
    setProfiles({ ...profiles, [targetKey]: merged });
    try {
      await saveSiteProfile(targetKey, merged);
    } catch {
      showToast(t("site.saveFailed"), "error");
    }
  };

  const inherited =
    resolved.key && resolved.key !== host ? <span class="fd-hint">{t("site.inheritedFrom", { host: resolved.key })}</span> : null;
  const form = <SiteProfileForm value={value} onChange={(v) => void save(v)} onCommitDir={(v) => void save(v)} />;

  if (props.collapsible) {
    const interceptLabel = {
      default: t("site.interceptDefault"),
      never: t("site.interceptNever"),
      always: t("site.interceptAlways"),
    }[value.intercept];
    return (
      <div class={`fd-site-card fd-collapsible${open ? " fd-open" : ""}`}>
        <button
          type="button"
          class="fd-site-toggle"
          aria-expanded={open}
          title={t("sidepanel.siteSettings")}
          onClick={() => setOpen(!open)}
        >
          <Icon name="globe" size={13} />
          <span class="fd-site-host">{targetKey}</span>
          {value.sniff ? null : <span class="fd-site-chip fd-warn">{t("site.sniffOff")}</span>}
          <span class="fd-site-chip">{interceptLabel}</span>
          <Icon name={open ? "chevronDown" : "chevronRight"} size={12} />
        </button>
        {open ? (
          <div class="fd-site-body">
            {inherited}
            {form}
          </div>
        ) : null}
      </div>
    );
  }

  return (
    <div class="fd-site-card">
      <div class="fd-site-head">
        <span class="fd-site-host" title={targetKey}>
          {targetKey}
        </span>
        {inherited}
      </div>
      {form}
    </div>
  );
}
