#!/bin/bash
# The Rust view of libvpx, generated once from the real headers and kept in
# the repository.
#
# Generated rather than written by hand because a struct written by hand is a
# struct that can be wrong in a way that corrupts memory, and generated ONCE
# rather than at every build because generating it needs clang -- a tool this
# project does not otherwise ask anybody to install.
#
# Only what the encoder uses is asked for, so the file stays readable.
#
#   bash tools/vpx-bindings.sh   (on a system with libvpx-dev and bindgen)
set -e
export PATH=/home/dev/.cargo/bin:/usr/bin:/bin
here="$(cd "$(dirname "$0")/.." && pwd)"
out="$here/crates/core/src/vpx.rs"

bindgen /usr/include/vpx/vp8cx.h \
  --allowlist-function 'vpx_codec_enc_config_default' \
  --allowlist-function 'vpx_codec_enc_init_ver' \
  --allowlist-function 'vpx_codec_encode' \
  --allowlist-function 'vpx_codec_get_cx_data' \
  --allowlist-function 'vpx_codec_destroy' \
  --allowlist-function 'vpx_codec_error_detail' \
  --allowlist-function 'vpx_codec_err_to_string' \
  --allowlist-function 'vpx_codec_vp8_cx' \
  --allowlist-function 'vpx_img_wrap' \
  --allowlist-function 'vpx_codec_control_' \
  --allowlist-type 'vpx_codec_enc_cfg' \
  --allowlist-type 'vpx_codec_ctx' \
  --allowlist-type 'vpx_codec_cx_pkt' \
  --allowlist-type 'vpx_image' \
  --allowlist-type 'vpx_codec_err_t' \
  --allowlist-var 'VPX_IMG_FMT_I420' \
  --allowlist-var 'VPX_EFLAG_FORCE_KF' \
  --allowlist-var 'VPX_DL_REALTIME' \
  --allowlist-var 'VPX_ENCODER_ABI_VERSION' \
  --allowlist-var 'VPX_CODEC_OK' \
  --allowlist-var 'VPX_FRAME_IS_KEY' \
  --allowlist-var 'VPX_RC_.*' \
  --allowlist-var 'VPX_KF_.*' \
  --no-layout-tests \
  --no-doc-comments \
  --raw-line '//! The Rust view of libvpx, generated from its headers and kept here.' \
  --raw-line '//!' \
  --raw-line '//! Generated, because a structure written out by hand is one that can be' \
  --raw-line '//! wrong in a way that corrupts memory rather than failing. Generated ONCE' \
  --raw-line '//! and committed, because generating it needs clang, and this project does' \
  --raw-line '//! not ask anybody to install clang to build it.' \
  --raw-line '//!' \
  --raw-line '//! From libvpx 1.16 headers. A system library of another version is caught' \
  --raw-line '//! at the door: the encoder is opened with the ABI number this was made' \
  --raw-line '//! against, and libvpx refuses a number it does not know rather than' \
  --raw-line '//! reading the wrong bytes.' \
  --raw-line '//!' \
  --raw-line '//! Remade with tools/vpx-bindings.sh.' \
  --raw-line '#![allow(non_upper_case_globals, non_camel_case_types, non_snake_case, dead_code)]' \
  -o "$out" -- -I/usr/include

wc -l "$out"
grep -c "pub fn vpx_" "$out"
