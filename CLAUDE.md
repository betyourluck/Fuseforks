# CLAUDE.md

## 台帳の構成（真実の記録先）

- 全体像・設計判断: [README.md](README.md)
- ドメイン契約（実装より先にここが正）: [data_contract.yaml](data_contract.yaml)
- 踏んだ罠（症状 → 真因 → 処方 → 一般化）: [failures.md](failures.md)
- 仕様: `specs/NN_*.md`（起票 → 査読 → rev 改訂 → Phase 分割で main 直コミット）

機構・enum・フィールド・API を変えたら、その名前で全台帳を grep して
追従漏れを回収するまで完了としない。

## Spec の状態

- Spec 01（sd / yq 書き換え系）〜 03（新規チャット + 広場ログ）: **Done**
- Spec 04（plan — 並列委譲と合流）: **Draft rev1 起票済み・査読待ち**（2026-07-30）
- Spec 05（Gemini ネイティブ経路と Google 検索による接地）:
  **rev2 査読承認 → Phase 0〜3 完了**（2026-07-29）。残は Phase 4（接地の来歴を
  イベントと表示層へ配線）と実機確認。本 Spec は例外的に実装先行
  （Gemini の実挙動が実測でしか決まらなかったため）
