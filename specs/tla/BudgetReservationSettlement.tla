---- MODULE BudgetReservationSettlement ----
\* 予約の清算が、見積もりと実測がずれても会計を壊さないか（Spec 38 P3）。
\*
\* 対象の実装:
\*   budget.rs `BudgetPool::try_reserve`   CAS で estimate を先に引く
\*   budget.rs `ReservationGuard::commit`  self を move で消費し差額を清算
\*   budget.rs `ReservationGuard::drop`    commit されなかったら全額返す
\*   turn.rs   `run_turn`                  予約 → LLM → commit。`?` は Drop へ落ちる
\*
\* BudgetOvershootBound は「境界で何本通るか」しか見ていない（CallCost が定数で、
\* 予約額と実費が常に一致する世界）。**このモデルは estimate ≠ actual を入れる** —
\* 過大予約（返金が要る）と過小予約（追加の debit が要る）の両方を非決定に
\* 起こし、あらゆる割り込み順序で会計が閉じるかを見る。
\*
\* 一番の的は **二重返金**。Spec 38 の承認査読 (a) が退けた設計
\* （`committed` フラグを持ち、commit が清算し Drop も返金する形）を
\* SpecDoubleRefund として並べ、**move セマンティクスを選んだ判断を機械で裏づける**。
\*
\* 回し方（`java` は PATH に無い。Android Studio 同梱の JBR を使う）:
\*   & "C:\Program Files\Android\Android Studio\jbr\bin\java.exe" `
\*       -XX:+UseParallelGC -cp extra\tla2tools.jar tlc2.TLC `
\*       -config BudgetReservationSettlement-double-refund.cfg `
\*       BudgetReservationSettlement.tla
\* **3 本を対で回す**（safe / double-refund / no-refund）。

EXTENDS Naturals, FiniteSets

CONSTANTS
    Agents,     \* 同じプールを指す個体（波の全タスク）
    Ceiling,    \* 天井
    Estimate,   \* 予約額（reserve_estimate_milli の出力。ここでは定数に単純化）
    Actuals,    \* 実測の取りうる値の集合。Estimate より小さい値と大きい値を混ぜる
    MaxCalls    \* TLC の環境を有限にするためだけの上限

ASSUME EstimateFits == Estimate <= Ceiling

VARIABLES
    remaining,  \* プールの残額
    reserved,   \* 個体 -> 予約中の額（飛行していなければ 0）
    spent,      \* 実際にプロバイダへ払った累計
    calls,      \* 個体 -> これまでに始めた呼び出し数
    saturated   \* 一度でも飽和（0 での張り付き / 天井での頭打ち）が起きたか

vars == <<remaining, reserved, spent, calls, saturated>>

\* 飽和つきの増減（u64 の実装に合わせる）。
SatSub(x, y) == IF x > y THEN x - y ELSE 0
CapAdd(x, y) == IF x + y > Ceiling THEN Ceiling ELSE x + y

RECURSIVE SumOver(_)
SumOver(S) ==
    IF S = {} THEN 0
    ELSE LET x == CHOOSE y \in S : TRUE
         IN reserved[x] + SumOver(S \ {x})

SumReserved == SumOver(Agents)

MaxPositiveError ==
    LET errs == {IF a > Estimate THEN a - Estimate ELSE 0 : a \in Actuals}
    IN CHOOSE m \in errs : \A e \in errs : e <= m

TypeOK ==
    /\ saturated \in BOOLEAN
    /\ remaining \in 0..Ceiling
    /\ reserved \in [Agents -> 0..Estimate]
    /\ calls \in [Agents -> 0..MaxCalls]
    /\ spent \in 0..(Ceiling + Cardinality(Agents) * MaxPositiveError)

Init ==
    /\ remaining = Ceiling
    /\ reserved = [a \in Agents |-> 0]
    /\ spent = 0
    /\ calls = [a \in Agents |-> 0]
    /\ saturated = FALSE

\* ── 予約（CAS。残額が足りなければ通さない）──────────────
Reserve(a) ==
    /\ reserved[a] = 0
    /\ calls[a] < MaxCalls
    /\ remaining >= Estimate
    /\ remaining # 0
    /\ remaining' = remaining - Estimate
    /\ reserved' = [reserved EXCEPT ![a] = Estimate]
    /\ calls' = [calls EXCEPT ![a] = @ + 1]
    /\ UNCHANGED <<spent, saturated>>

\* ── 清算（commit。self を move で消費するので Drop は走らない）──
\* actual > 予約 なら差額を引き、actual < 予約 なら差額を返す。
Commit(a) ==
    /\ reserved[a] # 0
    /\ \E cost \in Actuals :
        /\ spent' = spent + cost
        /\ remaining' =
            IF cost >= reserved[a]
            THEN SatSub(remaining, cost - reserved[a])
            ELSE CapAdd(remaining, reserved[a] - cost)
        \* 飽和が起きた瞬間に印を付ける。以後 AccountingHolds は問わない —
        \* 天井を超えて払った時点で「残額 = 天井 − 払い − 予約」は
        \* 実装として成立しえない（0 で張り付いた分は復元できない）。
        /\ saturated' = (saturated
            \/ (cost >= reserved[a] /\ remaining < cost - reserved[a])
            \/ (cost < reserved[a] /\ remaining + (reserved[a] - cost) > Ceiling))
    /\ reserved' = [reserved EXCEPT ![a] = 0]
    /\ UNCHANGED calls

\* ── commit されずに落ちた（`?` / abort → Drop が全額返す）──
\* 実費は発生していない扱い（発生していても usage が返らず清算できないのは
\* 予約の有無と独立の穴 = #103 の領域。ここでは会計だけを見る）。
Abort(a) ==
    /\ reserved[a] # 0
    /\ remaining' = CapAdd(remaining, reserved[a])
    /\ saturated' = (saturated \/ (remaining + reserved[a] > Ceiling))
    /\ reserved' = [reserved EXCEPT ![a] = 0]
    /\ UNCHANGED <<spent, calls>>

\* ── 壊れた版 1: 二重返金 ────────────────────────────────
\* commit が清算した**うえに** Drop も予約全額を返す
\* （`committed` フラグを持つ設計で、フラグの立て忘れ／立て損ないが起きた形）。
CommitThenAlsoRefund(a) ==
    /\ reserved[a] # 0
    /\ \E cost \in Actuals :
        /\ spent' = spent + cost
        /\ remaining' =
            CapAdd(
                IF cost >= reserved[a]
                THEN SatSub(remaining, cost - reserved[a])
                ELSE CapAdd(remaining, reserved[a] - cost),
                reserved[a])
        /\ saturated' = (saturated
            \/ (cost >= reserved[a] /\ remaining < cost - reserved[a]))
    /\ reserved' = [reserved EXCEPT ![a] = 0]
    /\ UNCHANGED calls

\* ── 壊れた版 2: 返し忘れ ────────────────────────────────
\* commit されずに落ちたのに Drop が何もしない（実装の Drop を空にした形）。
AbortWithoutRefund(a) ==
    /\ reserved[a] # 0
    /\ reserved' = [reserved EXCEPT ![a] = 0]
    /\ UNCHANGED <<remaining, spent, calls, saturated>>

NextSafe == \E a \in Agents : Reserve(a) \/ Commit(a) \/ Abort(a)
SpecSafe == Init /\ [][NextSafe]_vars

NextDoubleRefund == \E a \in Agents : Reserve(a) \/ CommitThenAlsoRefund(a) \/ Abort(a)
SpecDoubleRefund == Init /\ [][NextDoubleRefund]_vars

NextNoRefund == \E a \in Agents : Reserve(a) \/ Commit(a) \/ AbortWithoutRefund(a)
SpecNoRefund == Init /\ [][NextNoRefund]_vars

\* ── 会計（本モデルの主眼）──────────────────────────────
\* 天井を超えていない限り、残額・払い済み・予約中の 3 つで帳尻が合う。
\* 多く返せば remaining が大きくなり、返し忘れれば小さくなる — どちらも破れる。
\* **飽和が一度でも起きたら問わない。** 天井を超えて払った時点で 0 に
\* 張り付いた分は復元できず、以後の返金は帳尻を戻せない（実装の事実であって
\* 欠陥ではない）。飽和していない限り、多く返せば remaining が大きくなり、
\* 返し忘れれば小さくなる — 二重返金と返し忘れの両方がここで落ちる。
AccountingHolds ==
    (~saturated /\ spent + SumReserved <= Ceiling)
        => (remaining = Ceiling - spent - SumReserved)

\* 返金は天井を超えない（credit_milli の飽和）。
RemainingNeverExceedsCeiling ==
    remaining <= Ceiling

\* 超過の上限は「飛行中の正の見積もり誤差の総和」以下（Spec 38 の Goal）。
OvershootWithinEstimateError ==
    spent <= Ceiling + Cardinality(Agents) * MaxPositiveError

====
