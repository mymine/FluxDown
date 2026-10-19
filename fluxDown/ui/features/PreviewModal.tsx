/**
 * 资源预览弹层（合并原 popup / 页内面板两份实现）。
 * 按类型分发：图片 / 音频 / 视频直链 / HLS·DASH·分片；原生播放失败诚实降级，禁止引入 hls.js。
 */
import type { JSX } from "preact";
import { useRef, useState } from "preact/hooks";
import type { ResourceType } from "@/utils/resource-types";
import { Modal } from "@/ui/components/Modal";
import { useT } from "@/ui/i18n";
import "./features.css";

export interface PreviewTarget {
  url: string;
  title: string;
  type: ResourceType;
  mimeType?: string;
}

type PreviewKind = "image" | "audio" | "direct-video" | "hls" | "dash" | "fragment" | "unsupported";

/** 仅这些类型显示预览入口。 */
export function isPreviewable(type: ResourceType): boolean {
  return type === "image" || type === "video" || type === "audio" || type === "stream";
}

function previewKind(r: PreviewTarget): PreviewKind {
  const mime = r.mimeType?.toLowerCase() ?? "";
  const url = r.url.toLowerCase();
  if (r.type === "image" || mime.startsWith("image/")) return "image";
  if (r.type === "stream") {
    if (url.includes(".m3u8")) return "hls";
    if (url.includes(".mpd")) return "dash";
    return "fragment";
  }
  if (r.type === "audio") return "audio";
  if (r.type === "video") return "direct-video";
  return "unsupported";
}

/** 关闭时销毁 video/audio 释放网络连接。 */
function releaseMedia(el: HTMLMediaElement | null): void {
  if (!el) return;
  el.pause();
  el.removeAttribute("src");
  el.load();
}

export function PreviewModal(props: { target: PreviewTarget; onClose: () => void }): JSX.Element {
  const t = useT();
  const { target } = props;
  const kind = previewKind(target);
  const [failed, setFailed] = useState(false);
  const [dims, setDims] = useState("");
  const mediaRef = useRef<HTMLMediaElement | null>(null);

  const failMsg =
    kind === "hls"
      ? t("panel.previewHlsUnsupported")
      : kind === "dash"
        ? t("panel.previewDashUnsupported")
        : kind === "fragment"
          ? t("panel.previewFragmentUnsupported")
          : t("panel.previewFailed");

  let body: JSX.Element;
  if (kind === "unsupported") {
    body = <div class="fd-preview-fallback">{t("panel.previewUnsupported")}</div>;
  } else if (failed) {
    body = <div class="fd-preview-fallback">{failMsg}</div>;
  } else if (kind === "image") {
    body = (
      <>
        <img
          class="fd-preview-media"
          src={target.url}
          onLoad={(e) => {
            const img = e.currentTarget as HTMLImageElement;
            setDims(`${img.naturalWidth} × ${img.naturalHeight}`);
          }}
          onError={() => setFailed(true)}
        />
        {dims ? <div class="fd-preview-hint">{dims}</div> : null}
      </>
    );
  } else if (kind === "audio") {
    body = (
      <audio
        class="fd-preview-media"
        controls
        src={target.url}
        ref={mediaRef as { current: HTMLAudioElement | null }}
        onError={() => setFailed(true)}
      />
    );
  } else {
    body = (
      <video
        class="fd-preview-media"
        controls
        autoplay={kind === "direct-video"}
        muted={kind === "direct-video"}
        src={target.url}
        ref={mediaRef as { current: HTMLVideoElement | null }}
        onError={() => setFailed(true)}
      />
    );
  }

  const close = () => {
    releaseMedia(mediaRef.current);
    props.onClose();
  };

  return (
    <Modal title={target.title || t("panel.previewTitle")} onClose={close} width={640}>
      <div class="fd-preview-body">{body}</div>
    </Modal>
  );
}
