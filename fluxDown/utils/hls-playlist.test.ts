import { describe, expect, test } from "bun:test";
import { normalizeHlsPlaylist, parseHlsPlaylist } from "./hls-playlist";

const MASTER = `#EXTM3U
#EXT-X-MEDIA:TYPE=AUDIO,GROUP-ID="aac",NAME="en",URI="audio/en.m3u8"
#EXT-X-STREAM-INF:BANDWIDTH=2000000,RESOLUTION=1280x720,AUDIO="aac"
url_0/720p.m3u8
#EXT-X-STREAM-INF:BANDWIDTH=5000000,RESOLUTION=1920x1080,AUDIO="aac"
https://cdn2.example.com/x/url_8/1080p.m3u8
#EXT-X-I-FRAME-STREAM-INF:BANDWIDTH=90000,URI="iframe/index.m3u8"
`;

const MEDIA = `#EXTM3U
#EXT-X-TARGETDURATION:10
#EXT-X-MAP:URI="init/init.mp4"
#EXTINF:10.0,
url_462/seg_1.ts
#EXTINF:10.0,
url_463/seg_2.ts
#EXTINF:10.0,
https://edge.example.com/abs/seg_3.ts?token=1
#EXT-X-ENDLIST
`;

describe("parseHlsPlaylist", () => {
  test("主播放列表：子列表含各档视频、独立音轨与 I 帧列表，全部绝对化", () => {
    const info = parseHlsPlaylist(MASTER, "https://cdn.example.com/x/master.m3u8");
    expect(info?.children).toEqual([
      "https://cdn.example.com/x/audio/en.m3u8",
      "https://cdn.example.com/x/url_0/720p.m3u8",
      "https://cdn2.example.com/x/url_8/1080p.m3u8",
      "https://cdn.example.com/x/iframe/index.m3u8",
    ]);
    expect(info?.segmentDirs).toEqual([]);
  });

  test("媒体播放列表：分片目录（含 EXT-X-MAP 初始化段、跨域绝对 URL）去重", () => {
    const info = parseHlsPlaylist(MEDIA, "https://cdn.example.com/x/url_0/720p.m3u8");
    expect(info?.children).toEqual([]);
    expect(info?.segmentDirs).toEqual([
      "https://cdn.example.com/x/url_0/init/",
      "https://cdn.example.com/x/url_0/url_462/",
      "https://cdn.example.com/x/url_0/url_463/",
      "https://edge.example.com/abs/",
    ]);
  });

  test("不是 #EXTM3U 开头返回 null", () => {
    expect(parseHlsPlaylist("<html></html>", "https://cdn.example.com/a.m3u8")).toBeNull();
  });

  test("主播放列表的各档属性（带宽 / 分辨率 / 帧率 / 编码 / 音频组）", () => {
    const info = parseHlsPlaylist(MASTER, "https://cdn.example.com/x/master.m3u8");
    expect(info?.variants).toEqual([
      { url: "https://cdn.example.com/x/url_0/720p.m3u8", bandwidth: 2000000, width: 1280, height: 720, audioGroup: true },
      { url: "https://cdn2.example.com/x/url_8/1080p.m3u8", bandwidth: 5000000, width: 1920, height: 1080, audioGroup: true },
    ]);
  });

  test("SAMPLE-AES / 非 identity KEYFORMAT 视为 DRM；AES-128 不算", () => {
    const aes = "#EXTM3U\n#EXT-X-KEY:METHOD=AES-128,URI=\"k.bin\"\n#EXTINF:6,\nseg.ts\n";
    const fairplay = "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES,URI=\"skd://x\",KEYFORMAT=\"com.apple.streamingkeydelivery\"\n#EXTINF:6,\nseg.ts\n";
    const widevine = "#EXTM3U\n#EXT-X-KEY:METHOD=SAMPLE-AES-CTR,KEYFORMAT=\"urn:uuid:edef8ba9-79d6-4ace-a3c8-27dcd51d21ed\"\n#EXTINF:6,\nseg.ts\n";
    expect(parseHlsPlaylist(aes, "https://c.example.com/a.m3u8")?.protected).toBeUndefined();
    expect(parseHlsPlaylist(fairplay, "https://c.example.com/a.m3u8")?.protected).toBe(true);
    expect(parseHlsPlaylist(widevine, "https://c.example.com/a.m3u8")?.protected).toBe(true);
  });

  test("每个分片单独一个目录时收敛为公共目录前缀；前缀退化到站点根时不收敛", () => {
    const perSegment = ["#EXTM3U", ...Array.from({ length: 40 }, (_, i) => `#EXTINF:2,\nurl_${500 + i}/seg.ts`)].join("\n");
    expect(parseHlsPlaylist(perSegment, "https://cdn.example.com/x/url_8/media.m3u8")?.segmentDirs)
      .toEqual(["https://cdn.example.com/x/url_8/"]);
    const atRoot = ["#EXTM3U", ...Array.from({ length: 40 }, (_, i) => `#EXTINF:2,\n/d${i}/seg.ts`)].join("\n");
    expect(parseHlsPlaylist(atRoot, "https://cdn.example.com/media.m3u8")?.segmentDirs).toHaveLength(16);
  });
});

describe("normalizeHlsPlaylist", () => {
  test("丢弃非 http(s) 与非字符串项；全空返回 null", () => {
    expect(normalizeHlsPlaylist({ children: ["javascript:alert(1)", 3, "https://a.com/b.m3u8"], segmentDirs: "x" }))
      .toEqual({ children: ["https://a.com/b.m3u8"], segmentDirs: [] });
    expect(normalizeHlsPlaylist({ children: [], segmentDirs: [] })).toBeNull();
    expect(normalizeHlsPlaylist(null)).toBeNull();
  });
});
