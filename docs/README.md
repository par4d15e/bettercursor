# 文档布局

## 版本库内（日常开发 / 协作）

| 文件 | 用途 |
|------|------|
| [README.md](../README.md) | 入门、功能状态、安装与踩坑 |
| [PRD.md](../PRD.md) | 产品需求与验收矩阵 |
| [SYNC_DESIGN.md](../SYNC_DESIGN.md) | v0.2+ 同步与跨设备设计 |
| [AGENTS.md](../AGENTS.md) | 仓库规范（AI / 贡献者必读） |

## 本地归档 `docs/local/`（已 gitignore）

Agent handoff、项目考古、已完成的迁移计划、E2E 操作笔记等 **不参与日常开发** 的材料放在此目录，默认不提交到远端。

可从 git 历史恢复，或自行维护本地副本：

- `BACKGROUND.md` — 调研与代际考古
- `TAURI_RUST_PLAN.md` — Python → Rust 迁移计划（已 largely 落地）
- `HANDOFF.md` — milestone 交接笔记
- `CROSS_DEVICE_E2E.md` — 跨设备手动验收步骤
