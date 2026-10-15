#!/usr/bin/env bash
#
# 打 Linux 便携包（tar.gz）。
#
# 便携布局的依据见 crates/ling-chat-main/src/data_dir.rs：发布版把 data 目录放在
# 可执行文件旁边。tauri-build 在编译阶段已经把 bundle.resources 复制到了
# target/release/ 下（data/.official、data/third_party），因此这里只需把产物
# 收拢成「exe 与 data 同级」的目录再压缩。
#
# 用法:
#   bash scripts/package-linux-portable.sh [target_dir] [out_dir]
# 默认: target/release 与 dist-portable

set -euo pipefail

TARGET_DIR="${1:-target/release}"
OUT_DIR="${2:-dist-portable}"

VERSION="$(node -p "require('./src-tauri/tauri.conf.json').version")"
ARCH="$(uname -m)"
PKG_NAME="LingChat-${VERSION}-linux-${ARCH}"
STAGE="${OUT_DIR}/${PKG_NAME}"

if [ ! -x "${TARGET_DIR}/ling_chat" ]; then
  echo "找不到可执行文件 ${TARGET_DIR}/ling_chat（请先执行 tauri build）" >&2
  exit 1
fi
if [ ! -d "${TARGET_DIR}/data" ]; then
  echo "找不到 ${TARGET_DIR}/data（bundle.resources 未被复制，便携包会缺少内置资源）" >&2
  exit 1
fi

rm -rf "${STAGE}"
mkdir -p "${STAGE}"

cp "${TARGET_DIR}/ling_chat" "${STAGE}/ling_chat"
chmod +x "${STAGE}/ling_chat"

# 内置资源：首次启动会从 data/.official/ 播种到 data/ 后删除它
cp -a "${TARGET_DIR}/data" "${STAGE}/data"

# ort webgpu 的 Dawn 动态库（目前仅 linux-amd64 生成）
if [ -f "${TARGET_DIR}/libwebgpu_dawn.so" ]; then
  cp -a "${TARGET_DIR}/libwebgpu_dawn.so" "${STAGE}/"
fi

cat > "${STAGE}/README.txt" <<'EOF'
LingChat Linux 便携版
=====================

运行：
    ./ling_chat

本目录是便携布局：程序的数据目录就是它旁边的 data/。首次启动会把
data/.official/ 中的内置资源播种进来，之后的存档、日志、语音也都写在 data/ 下。
整个目录可以随意移动或改名，只要 ling_chat 与 data/ 保持同级即可。

NVIDIA 显卡用户：
程序启动时会自动检测显卡与桌面会话，并在需要时写入 WebKitGTK 的兼容环境
变量（详情见启动日志中的 [WebKit] 行）。如需关闭该自动处理：

    LINGCHAT_WEBKIT_QUIRK=0 ./ling_chat

系统依赖：
需要已安装 WebKitGTK 4.1 运行库（Debian/Ubuntu 为 libwebkit2gtk-4.1-0）。
EOF

tar -czf "${OUT_DIR}/${PKG_NAME}.tar.gz" -C "${OUT_DIR}" "${PKG_NAME}"
rm -rf "${STAGE}"

echo "已生成 ${OUT_DIR}/${PKG_NAME}.tar.gz"
ls -lh "${OUT_DIR}/${PKG_NAME}.tar.gz"
