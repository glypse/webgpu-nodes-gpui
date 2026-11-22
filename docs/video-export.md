# Video export: spec

**Decision (2026-11-22):** export offers only FFmpeg's built-in encoders: ProRes, FFV1, DNxHD/DNxHR, PNG and QuickTime Animation, in MOV and MKV. No H.264, H.265, VP9 or AV1, and no external encoder crates. The default is ProRes 422 HQ in MOV. Sections 2 and 3 below are the research that led here; where they propose openh264 or rav1e, this decision overrides them.

Sources: Blender `main` (fetched 2026-11-22), `source/blender/imbuf/movie/intern/movie_write.cc`, `movie_util.cc`, `MOV_enums.hh`, `source/blender/makesdna/DNA_scene_types.h`, the RNA is `source/blender/makesrna/intern/rna_scene.cc`, the UI is `scripts/startup/bl_ui/properties_output.py`, presets are `scripts/presets/ffmpeg/`, and FFmpeg's encoder list is `libavcodec/allcodecs.c` on FFmpeg master.

## 1. Blender's model

**Containers** (RNA `format`, default MKV): MPEG-4 (.mp4), Matroska (.mkv), WebM (.webm); legacy: AVI, QuickTime (.mov), DV, Flash, MPEG-1, MPEG-2, Ogg.

**Codec shown for** AVI, QuickTime, MKV, Ogg, MPEG-4, WebM. The legacy containers fix the codec in the writer (`movie_write.cc`, the `ffmpeg_type` switch): MPEG-1, MPEG-2, DV, Flash, Ogg (Theora), and the H.264/Xvid/AV1 presets.

**Video codecs** (RNA `codec`, default H.264): AV1, H.264, H.265, WebM (VP9), then DNxHD, DV, FFV1, Flash, HuffYUV, MPEG-1, MPEG-2, MPEG-4, PNG, ProRes, QuickTime Animation (QTRLE), Theora. "No Video" is also an option.

**Output Quality** (RNA `constant_rate_factor`, default Medium). Values are the x264 CRF scale; the UI shows it only for H.264, H.265, MPEG-4, WebM and AV1 (`MOV_codec_supports_crf`):

| Preset | H.264 CRF | AV1 libaom | AV1 SVT |
|---|---|---|---|
| Lossless | 0 | 0 | qp 0 |
| Perceptually Lossless | 17 | 15 | 17 |
| High | 20 | 21 | 25 |
| Medium (default) | 23 | 27 | 31 |
| Low | 26 | 33 | 36 |
| Very Low | 29 | 38 | 42 |
| Lowest | 32 | 44 | 49 |
| Custom | 0 to 63 (`custom_constant_rate_factor`, default 23) | | |

- The AV1 columns are remaps applied when the preset (not Custom) is used. VP9 and H.264/H.265 use the CRF value directly, so VP9's 0 to 63 scale starts at the same numbers.
- Clamp for custom values: 0 to 51 for H.264, H.265 and AV1; 0 to 63 for VP9.
- Constant Bitrate (NONE) is a separate mode, not a CRF value. Choosing a codec without CRF support resets quality to Constant Bitrate.
- Lossless: H.264 and H.265 and AV1 and VP9 switch to 4:4:4 (`YUV444P`) whenever CRF is 0; H.265 uses x265 `lossless=1`, VP9 `lossless=1`, AV1 SVT `qp=0`, H.264 10-bit `qp=0`.

**Encoding Speed** (RNA `ffmpeg_preset`, default Good). Labels: Slowest (`BEST`), Good (`GOOD`), Realtime (`REALTIME`). Mapping in the writer:

- H.264 (x264): `slower` / `medium` / `superfast` preset.
- VP9: `deadline` best / good / realtime.
- AV1 SVT-AV1: `preset` 3 / 5 / 8.
- AV1 libaom: `cpu-used` 4 / 6 / 8.
- Hidden for DNxHD, FFV1, HuffYUV, PNG, ProRes, QTRLE.

**Bitrate and rate control** (shown only in Constant Bitrate mode, or for codecs without CRF): `video_bitrate` (kbit/s; written as `bit_rate = value*1000`), `minrate` and `maxrate` (kbit/s, hidden for DNxHD), `buffersize` (kB, written `*1024`), `muxrate` (bit/s) and `packetsize` (bytes). Hidden for FFV1, HuffYUV, PNG, ProRes, QTRLE.

**GOP and B-frames**: `gopsize` (default 25, range 0 to 500; hidden for DNxHD, HuffYUV, PNG, ProRes). `use_max_b_frames` plus `max_b_frames` (0 to 16); hidden for FFV1 and QTRLE too. The FFmpeg default applies when the flag is off.

**Codec-specific**:
- ProRes profile (default 422 Standard): Proxy, LT, 422, 422 HQ, 4444, 4444 XQ. The writer uses `prores_ks` for RGBA and `prores_aw` otherwise, both native.
- DNxHD "Lossless output" sets `mb_decision = 2`.
- Presets in `scripts/presets/ffmpeg/`: H.264 in MP4 and in Matroska (GOP 15/18, 6000 kbit/s, max 9000, min 0, buffer 1792 kB, packet 2048, mux 10080000), H.264 for scrubbing (GOP 1, Perceptually Lossless, B-frames 0), Theora in Ogg, WebM (VP9 + Opus, CRF Medium), Xvid, DVD.

**Colour depth, pixel formats and alpha**:

| Codec | 8-bit | 10-bit | 12-bit | 16-bit | Alpha |
|---|---|---|---|---|---|
| H.264 | yuv420p | yuv420p10le | | | no |
| H.265 | yuv420p | yuv420p10le | yuv420p12le | | no |
| AV1 | yuv420p | yuv420p10le | yuv420p12le | | no in Blender's list |
| VP9 | yuv420p, RGBA gives yuva420p | | | | yes |
| ProRes 422 | yuv422p10le (fixed) | | | | no |
| ProRes 4444 | yuv444p10le | | | | yuva444p10le |
| FFV1 | gray8 / gbrp / gbrap | gray10 / gbrp10 / gbrap10 | 12-bit variants | 16-bit variants | yes |
| PNG, QTRLE, HuffYUV | rgb24 / rgba / argb / bgra | | | | yes |

- Colour depth is shown for H.264, H.265, AV1, ProRes and FFV1 only.
- HDR (`is_hdr`) requires H.265 or AV1 and 10 or 12 bits.
- DNxHD, MPEG and others use the encoder's first supported format (`ffmpeg_get_pix_fmts`).

**Colour tagging** (`set_colorspace_options`): CICP primaries, transfer, matrix and range are taken from the display colour space via `IMB_colormanagement_space_to_cicp`. RGB formats get matrix RGB. Full range becomes JPEG range. If no CICP is known, the output is tagged unspecified primaries and transfer, BT.709 matrix, MPEG range. For BT.2020 primaries, mastering-display metadata is attached, with max luminance from the view transform (HLG capped at 1000 nits, PQ at 10000).

## 2. Constraints for this app

FFmpeg is built from source and statically linked (`ffmpeg-next` 9.1.0 / `ffmpeg-sys-next` 9.1.1, `build` feature), with no system libraries. Its `build-lib-*` features only link libraries already installed, so libx264, libx265, libvpx, libaom, libsvtav1, librav1e, libtheora and libopenh264 are not available unless built separately. Blender's FFmpeg enables them (`build_files/build_environment/cmake/ffmpeg.cmake`), which we cannot copy.

**Encoders FFmpeg builds in** (non-external entries in `allcodecs.c`):
- Video: `prores_ks`, `prores_aw`, `ffv1`, `dnxhd`, `dvvideo`, `huffyuv`, `ffvhuff`, `png`, `qtrle`, `utvideo`, `mjpeg`, `mpeg1video`, `mpeg2video`, `mpeg4`, `flv`, `gif`, `jpeg2000`, `exr`, `dpx`, `magicyuv`, `rawvideo`, `vc2`, `cfhd`, `hap`.
- Audio: `aac`, `flac`, `mp2`, `ac3`, `vorbis`, `pcm_*`, and the experimental native `opus`.
- Not built in: H.264, H.265, VP9, AV1 software encoding. FFmpeg has no native encoder for any of them.

**Pure-Rust or source-built encoders for the gaps**:
- **H.264: `openh264` 0.9.10** (BSD-2, updated 2026-11). Its default `source` feature compiles Cisco's OpenH264 with `cc`. The API exposes `Profile` (Baseline to High444), `RateControlMode::Quality` and a QP range. Limits: the OpenH264 encoder is used in practice for Constrained Baseline (no CABAC, no B-frames, no High 8x8 transform); verify High support before promising it. Input is 8-bit 4:2:0 I420 only, so no 10-bit H.264 and no 4:4:4 lossless. Cisco's published licence covers its prebuilt binary; whether the source build carries the same royalty terms needs a legal read before shipping. Rejected: `x264` crate (GPL, needs system x264).
- **AV1: `rav1e` 0.8.1** (BSD-2, 2025-06, pure Rust with x86 asm via the `asm` feature). Speed presets 0 to 10, quantizer and bitrate modes, 8/10-bit and 4:2:0 to 4:4:4 (confirm 12-bit before relying on it). Slower than SVT-AV1, so benchmark on the target scene. Rejected: `svt-av1-sys` 0.1.1 (2020, stale).
- **VP9: no usable crate.** `env-libvpx-sys` 5.1.3 (MPL-2.0, 2023) links a system libvpx; `vpx-sys` 0.1.1 is 2021. A custom `build.rs` compiling libvpx (BSD) is the only route. Recommendation: drop VP9 and use AV1 in WebM.
- **H.265: no crate.** Drop it. AV1 10-bit covers HDR.

**Muxing**: packets from `openh264` (Annex-B NALs) and `rav1e` (OBUs) are written through `ffmpeg-next`'s output context. MP4 needs an `avcC` (H.264) or `av1C` (AV1) extradata built from the SPS/PPS or sequence header; MKV and WebM accept Annex-B and low-overhead OBUs with that extradata. Confirm each muxer's accepted codecs at runtime; FFmpeg reports them.

**Hardware encoders** (VideoToolbox on macOS, NVENC, QSV, VAAPI, AMF, MF) need platform SDKs and are not in the built-in list. The current export uses AVFoundation (`crates/export`, macOS only); a cross-platform FFmpeg path would replace it. VideoToolbox could stay as a macOS-only option later.

## 3. Proposed export dialog menu

**Container**: MP4 (H.264, AAC), QuickTime (.mov, ProRes, AAC), Matroska (.mkv, any supported codec), WebM (.webm, AV1 only). Dropped: legacy containers (AVI, DV, Flash, MPEG-1/2, Ogg) and MPEG-4 part 2 in AVI; Blender's legacy formats do not add value and several need external encoders.

**Codec** (default H.264):
- H.264 (openh264, 8-bit, Constrained Baseline).
- AV1 (rav1e, 8/10-bit, HDR-capable).
- ProRes (native; 422 Proxy/LT/422/HQ/4444/4444 XQ; 10-bit; alpha in 4444).
- FFV1 (native, lossless, 8/10/12/16-bit, alpha).
- PNG and QuickTime Animation (native, lossless, alpha) as "Lossless with alpha".
- DNxHD (native, 8-bit 4:2:2; check its fixed resolution and frame-rate profiles).
- Dropped: H.265, VP9, Theora, MPEG-1/2, MPEG-4 part 2, DV, Flash, HuffYUV, all external-library codecs.

**Output quality**: Blender's labels, kept for H.264 and AV1. Quality values are not portable from CRF: openh264 uses QP, rav1e uses quantizer. Calibrate the mapping against Blender's output on a test scene by PSNR or VMAF, starting from the table above.
- Perceptually Lossless (default for H.264 and AV1).
- High, Medium, Low, Very Low, Lowest.
- Custom: 0 to 51 (H.264 QP), 0 to 255 (rav1e quantizer); the UI range depends on the codec.
- ProRes uses its profile instead; FFV1, PNG and QTRLE are lossless with no quality setting.
- Constant bitrate: a single target kbit/s (openh264 Bitrate mode, rav1e bitrate). Drop Blender's min/max/buffer/mux rate and packet size; check whether the encoders expose VBV controls before adding them.

**Encoding speed**: Slowest, Good (default), Realtime. Map to an openh264 complexity option (verify which one the crate exposes) and rav1e speed 3 / 6 / 10 (initial guesses; benchmark). Hidden for ProRes, FFV1, PNG and QTRLE, like Blender.

**Keyframe interval**: default 25, range 0 to 500, shown for H.264 and AV1 (rav1e keyframe interval; openh264 intra period; verify both parameter names). **B-frames**: dropped for H.264 (Constrained Baseline) and AV1 (rav1e lag-in-frames, if needed later).

**Colour depth**: 8-bit default. 10-bit for AV1 and ProRes; 12-bit only if rav1e confirms it. HDR toggle only with AV1 10-bit.

**Colour tagging**: mirror Blender's CICP tagging from the display colour space; add mastering-display metadata for BT.2020 HDR.

**Alpha**: ProRes 4444, PNG and QTRLE, and FFV1 keep RGBA. H.264 and AV1 ignore alpha (as Blender's list does).

**Defaults**: MP4, H.264, Perceptually Lossless, Good, keyframe interval 25, 8-bit BT.709 tagging. This is visually lossless for the expected content; confirm it on a test scene before shipping.

**Open decisions**:
1. openh264 Baseline may be too weak for the quality target; compare against an H.264 High encode before committing.
2. rav1e speed is the main export-time risk; measure before setting the Realtime preset.
3. Patent and licence review for openh264 source builds and for AV1 in general.
