---- MODULE BudgetOvershootBound ----
\* Spec 11 の天井を、波が N 体へ同時に撒いたときにどれだけ超えるか。
\*
\* 対象の実装:
\*   budget.rs `BudgetPool::try_reserve`  load のみ（予約しない）
\*   turn.rs `run_turn` の周回境界  try_reserve → LLM → debit
\*   delegation.rs `run_plan`
\*       波は JoinSet で N 体を spawn し、全タスクが **同一の Arc<BudgetPool>** を指す
\*
\* `run_turn` の旧注釈は「飛行中 **1 呼び出し分**のオーバーシュートは許容」と
\* 書いている。このモデルはその主張を不変条件 OvershootAtMostOneCall として置き、
\* 現行の形（SpecCurrent）で破れるかを機械に出させる。
\*
\* MASC の bug-model とは向きが逆で、**壊れた版のほうが出荷済みのコード**。
\* だから -current.cfg が赤くなることが「実装が主張を満たしていない」の証拠になり、
\* .cfg のほうを緩めて通してはならない。
\*
\* 単純化: 1 呼び出しの実費を定数 CallCost にしている。実際は可変なので、
\* 出てくる上限は「同時に飛べる本数 × その時いちばん大きい 1 呼び出し」と読む。
\*
\* 回し方（`java` は PATH に無い。Android Studio 同梱の JBR を使う）:
\*   & "C:\Program Files\Android\Android Studio\jbr\bin\java.exe" `
\*       -XX:+UseParallelGC -cp extra\tla2tools.jar tlc2.TLC `
\*       -config BudgetOvershootBound-current.cfg BudgetOvershootBound.tla

EXTENDS Naturals, FiniteSets

CONSTANTS
    Agents,     \* 波が同時に起こす個体の集合（全員が同じプールを指す）
    Ceiling,    \* 天井（1 呼び出しぶんを 1 と数えた単位）
    CallCost    \* 1 呼び出しの実費

VARIABLES
    remaining,  \* プールの残額
    inflight,   \* try_reserve を通ったが debit していない個体
    spent       \* 実際にプロバイダへ払った累計

vars == <<remaining, inflight, spent>>

TypeOK ==
    /\ remaining \in 0..Ceiling
    /\ inflight \subseteq Agents
    /\ spent \in 0..(Ceiling + Cardinality(Agents) * CallCost)

Init ==
    /\ remaining = Ceiling
    /\ inflight = {}
    /\ spent = 0

\* ── 現行（budget.rs の try_reserve は load のみ）──────────
\* 残額が 1 でも残っていれば、何体でも同時に通る。

ReserveByLoad(a) ==
    /\ a \notin inflight
    /\ remaining > 0
    /\ inflight' = inflight \cup {a}
    /\ UNCHANGED <<remaining, spent>>

\* ── 予約する形（fix 案）────────────────────────────────
\* try_reserve が CAS で見積もりぶんを先に引く。残額が足りなければ通さない。

ReserveByCas(a) ==
    /\ a \notin inflight
    /\ remaining >= CallCost
    /\ remaining' = remaining - CallCost
    /\ inflight' = inflight \cup {a}
    /\ UNCHANGED spent

\* ── 実費の確定（両方で共通）─────────────────────────────
\* LLM が返ってきて debit する。払いは必ず起きる（ここが #103 の隣）。

DebitByLoad(a) ==
    /\ a \in inflight
    /\ inflight' = inflight \ {a}
    /\ remaining' = IF remaining > CallCost THEN remaining - CallCost ELSE 0
    /\ spent' = spent + CallCost

DebitAfterCas(a) ==
    /\ a \in inflight
    /\ inflight' = inflight \ {a}
    /\ spent' = spent + CallCost
    /\ UNCHANGED remaining      \* 予約時に引き済み

NextCurrent ==
    \E a \in Agents : ReserveByLoad(a) \/ DebitByLoad(a)

SpecCurrent == Init /\ [][NextCurrent]_vars

NextReserving ==
    \E a \in Agents : ReserveByCas(a) \/ DebitAfterCas(a)

SpecReserving == Init /\ [][NextReserving]_vars

\* ── 台帳が主張していること ──────────────────────────────
\* `run_turn` の旧注釈「飛行中 1 呼び出し分のオーバーシュートは許容して数える」

OvershootAtMostOneCall ==
    spent <= Ceiling + CallCost

\* ── 実際に成り立つ上限（現行の形で保たれるほう）──────────
\* 同時に飛べる本数ぶんまで超える。

OvershootAtMostConcurrency ==
    spent <= Ceiling + Cardinality(Agents) * CallCost

====
