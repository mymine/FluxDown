import { describe, expect, test } from "bun:test";
import { buildOptions, collectStreams, finalizeStreams, parseCodecs, pickOption, toVariant } from "../src/formats.js";
import { urlParam } from "../src/query.js";

type Fmt = Record<string, unknown>;
const gv = (itag: number, extra = "") => `https://rr1---sn-x.googlevideo.com/videoplayback?itag=${itag}&n=nnn${extra}`;
const video = (itag: number, height: number, mimeType: string, extra: Fmt = {}): Fmt => ({
  itag,
  url: gv(itag),
  mimeType,
  width: Math.round((height * 16) / 9),
  height,
  fps: 30,
  bitrate: height * 1000,
  contentLength: String(height * 10000),
  approxDurationMs: "200000",
  ...extra,
});
const audio = (itag: number, mimeType: string, extra: Fmt = {}): Fmt => ({
  itag,
  url: gv(itag),
  mimeType,
  bitrate: 128000,
  contentLength: "3000000",
  approxDurationMs: "200000",
  ...extra,
});
const response = (adaptiveFormats: Fmt[], formats: Fmt[] = []) => ({
  videoDetails: { videoId: "dQw4w9WgXcQ", lengthSeconds: "200" },
  streamingData: { formats, adaptiveFormats },
});
const identity = { n: { nnn: "solved-n" }, sig: {} };

describe("codec detection", () => {
  test("flags HDR the way yt-dlp's parse_codecs does", () => {
    expect(parseCodecs("vp09.02.51.10.01.09.16.09.00")).toEqual({ vcodec: "vp09.02.51.10.01.09.16.09.00", acodec: "none", dynamicRange: "HDR10" });
    expect(parseCodecs("av01.0.12M.10.0.110.09.16.09.0").dynamicRange).toBe("HDR10");
    expect(parseCodecs("av01.0.08M.08").dynamicRange).toBe("SDR");
    expect(parseCodecs("avc1.64001F, mp4a.40.2")).toEqual({ vcodec: "avc1.64001F", acodec: "mp4a.40.2", dynamicRange: "SDR" });
  });
});

describe("stream collection", () => {
  test("first client wins per itag/audio track/DRC and unusable formats are skipped", () => {
    const { streams, nChallenges, sigChallenges } = collectStreams([
      {
        client: "tv_samsung",
        response: response([
          video(137, 1080, 'video/mp4; codecs="avc1.640028"'),
          video(299, 1080, 'video/mp4; codecs="avc1.64002a"', { drmFamilies: ["WIDEVINE"] }),
          video(160, 144, 'video/mp4; codecs="avc1.4d400c"', { type: "FORMAT_STREAM_TYPE_OTF" }),
          video(400, 1440, 'video/mp4; codecs="av01.0.12M.08"', { targetDurationSec: 5 }),
          { itag: 248, mimeType: 'video/webm; codecs="vp9"', height: 1080 },
          {
            itag: 140,
            mimeType: 'audio/mp4; codecs="mp4a.40.2"',
            signatureCipher: `s=${encodeURIComponent("ABC=")}&sp=sig&url=${encodeURIComponent(gv(140))}`,
          },
        ]),
      },
      {
        client: "visionos",
        response: response([video(137, 1080, 'video/mp4; codecs="avc1.640028"', { url: "https://other/137" }), video(136, 720, 'video/mp4; codecs="avc1.4d401f"', { url: "https://other/136" })]),
      },
    ]);
    expect(streams.map((s) => `${s.client}:${(s.f as Fmt).itag}`)).toEqual(["tv_samsung:137", "tv_samsung:140", "visionos:136"]);
    expect(nChallenges).toEqual(["nnn"]);
    expect(sigChallenges).toEqual(["ABC="]);
  });

  test("applies solved signatures under the cipher's parameter name and drops unsolved formats", () => {
    const { streams } = collectStreams([
      {
        client: "tv_samsung",
        response: response([
          { itag: 140, mimeType: 'audio/mp4; codecs="mp4a.40.2"', signatureCipher: `s=ABC&sp=sig&url=${encodeURIComponent(gv(140))}` },
          { itag: 251, mimeType: 'audio/webm; codecs="opus"', signatureCipher: `s=XYZ&url=${encodeURIComponent(gv(251))}` },
          video(137, 1080, 'video/mp4; codecs="avc1.640028"', { url: gv(137).replace("n=nnn", "n=other") }),
        ]),
      },
    ]);
    const formats = finalizeStreams(streams, { n: { nnn: "fast" }, sig: { ABC: "CBA", XYZ: "ZYX" } });
    expect(formats.map((f) => f.itag)).toEqual([140, 251]);
    expect(urlParam(formats[0]!.url, "sig")).toBe("CBA");
    expect(urlParam(formats[0]!.url, "n")).toBe("fast");
    expect(urlParam(formats[1]!.url, "signature")).toBe("ZYX");
    expect(formats[0]).toMatchObject({ ext: "m4a", acodec: "mp4a.40.2", vcodec: "none", filesize: 0 });
  });
});

describe("option building", () => {
  const formats = finalizeStreams(
    collectStreams([
      {
        client: "tv_samsung",
        response: response(
          [
            video(401, 2160, 'video/mp4; codecs="av01.0.12M.08"'),
            video(313, 2160, 'video/webm; codecs="vp9"'),
            video(137, 1080, 'video/mp4; codecs="avc1.640028"'),
            video(399, 1080, 'video/mp4; codecs="av01.0.08M.08"', { bitrate: 9e6 }),
            video(248, 1080, 'video/webm; codecs="vp9"', { bitrate: 8e6 }),
            video(616, 1080, 'video/mp4; codecs="avc1.640028"', { url: gv(616, "&xtags=sr%3D1") }),
            audio(140, 'audio/mp4; codecs="mp4a.40.2"', { audioTrack: { id: "en.4", displayName: "English", audioIsDefault: true } }),
            audio(141, 'audio/mp4; codecs="mp4a.40.2"', { bitrate: 256000, audioTrack: { id: "ja.3", displayName: "Japanese original", audioIsDefault: false } }),
            audio(251, 'audio/webm; codecs="opus"', { bitrate: 160000, audioTrack: { id: "ja.3", displayName: "Japanese original" } }),
            audio(258, 'audio/mp4; codecs="mp4a.40.2"', { isDrc: true, bitrate: 400000, audioTrack: { id: "ja.3", displayName: "Japanese original" } }),
            video(18, 360, 'video/mp4; codecs="avc1.42001E, mp4a.40.2"', { approxDurationMs: "50000" }),
          ],
          [video(22, 720, 'video/mp4; codecs="avc1.64001F, mp4a.40.2"')],
        ),
      },
    ]).streams,
    identity,
  );

  test("compat prefers H.264 + AAC MP4 at the same height and pairs the original-language track", () => {
    const options = buildOptions(formats, "compat", "Title");
    expect(options.map((o) => o.label)).toEqual(["2160p · AV1 · MP4", "1080p · H.264 · MP4", "720p · H.264 · MP4", "仅音频 · AAC · M4A"]);
    expect(options[1]!.video.itag).toBe(137);
    expect(options[1]!.audio?.itag).toBe(141);
    expect(options[2]).toMatchObject({ audio: null, fileName: "Title.mp4" });
    expect(options[3]).toMatchObject({ fileName: "Title.m4a" });
    expect(options[3]!.video.itag).toBe(141);
  });

  test("quality prefers AV1/VP9 and falls back to MKV when the container differs", () => {
    const options = buildOptions(formats, "quality", "T");
    expect(options[1]!.video.itag).toBe(399);
    expect(options[1]!.audio?.itag).toBe(141);
    expect(options[0]!.video.itag).toBe(401);
    const webmOnly = buildOptions(
      formats.filter((f) => f.itag === 248 || f.itag === 141),
      "compat",
      "T",
    );
    expect(webmOnly[0]).toMatchObject({ container: "mkv", fileName: "T.mkv" });
  });

  test("fixed presets pick the highest option at or below the cap, or the lowest one", () => {
    const options = buildOptions(formats, "compat", "T");
    expect(pickOption(options, "1440")!.height).toBe(1080);
    expect(pickOption(options, "480")!.height).toBe(720);
    expect(pickOption(options, "audio")!.height).toBe(0);
    expect(pickOption(options, "ask")!.height).toBe(2160);
  });

  test("separate tracks beat a muxed format at the same height, and sizes are exact or unknown", () => {
    const muxedFirst = finalizeStreams(
      collectStreams([
        {
          client: "tv_samsung",
          response: response(
            [video(134, 360, 'video/mp4; codecs="avc1.4d401e"', { bitrate: 300000 }), audio(140, 'audio/mp4; codecs="mp4a.40.2"')],
            [video(18, 360, 'video/mp4; codecs="avc1.42001E, mp4a.40.2"', { bitrate: 500000, contentLength: undefined })],
          ),
        },
      ]).streams,
      identity,
    );
    const [p360] = buildOptions(muxedFirst, "compat", "T");
    expect([p360!.video.itag, p360!.audio?.itag]).toEqual([134, 140]);
    expect(toVariant(p360!).totalBytes).toBe(360 * 10000 + 3000000);

    // 没有独立音轨时只剩音画合一的格式；它没有 contentLength，大小报 0（未知）而不是估算值。
    const [muxed] = buildOptions(
      muxedFirst.filter((f) => f.itag === 18),
      "compat",
      "T",
    );
    expect(muxed!.video.itag).toBe(18);
    expect(toVariant(muxed!).totalBytes).toBe(0);
  });
});
