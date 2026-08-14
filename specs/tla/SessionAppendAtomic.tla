---- MODULE SessionAppendAtomic ----
\* Bug Model: sessions.redb の追記が 2 つに割れると、再起動した読み手が
\* 「レコードはあるが meta が数えていない」状態を観測する。
\*
\* 対象の実装: crates/fuseforks-core/src/session_store.rs の `append`。
\* 現行は RECORDS への insert と SESSIONS(meta) の更新を **1 つの
\* begin_write で囲み、commit を 1 回だけ呼ぶ**。
\* このモデルが留めるのは「その 1 トランザクションを割ってはならない」で、
\* 割った版（BuggyWriteRecord → BuggyWriteMeta）を並べて、割ると何が
\* 観測されるかを機械に出させる。
\*
\* 読み手はプロセスをまたぐ。redb の未 commit な状態はディスクに無いので、
\* 観測できるのは commit 済みの値だけ。だから Read は disk_* しか見ない。
\*
\* 回し方（`java` は PATH に無い。JDK は Android Studio 同梱の JBR を使う）:
\*   & "C:\Program Files\Android\Android Studio\jbr\bin\java.exe" `
\*       -XX:+UseParallelGC -cp extra\tla2tools.jar tlc2.TLC `
\*       -config SessionAppendAtomic-buggy.cfg SessionAppendAtomic.tla
\* TLC は cwd に states\ を掘るので、リポジトリ直下では回さない
\* （scratchpad へ .tla と .cfg を写してから回す）。
\* **2 本を対で回す。** 片方だけでは判別しているかどうかが分からない。

EXTENDS Naturals

VARIABLES
    disk_records,   \* 0 | 1 : レコードが永続化されたか
    disk_meta,      \* 0 | 1 : meta.record_count がそれを数えたか
    writer_phase,   \* "idle" | "record_committed" | "done"
    reader_result   \* "none" | "consistent" | "torn"

vars == <<disk_records, disk_meta, writer_phase, reader_result>>

Init ==
    /\ disk_records = 0
    /\ disk_meta = 0
    /\ writer_phase = "idle"
    /\ reader_result = "none"

\* ── 割れた追記（2 トランザクション）────────────────────
\* 「meta の更新はまとめて後で」と最適化した将来の版がこの形になる。

BuggyWriteRecord ==
    /\ writer_phase = "idle"
    /\ writer_phase' = "record_committed"
    /\ disk_records' = 1
    /\ UNCHANGED <<disk_meta, reader_result>>

BuggyWriteMeta ==
    /\ writer_phase = "record_committed"
    /\ writer_phase' = "done"
    /\ disk_meta' = 1
    /\ UNCHANGED <<disk_records, reader_result>>

\* ── 現行の追記（1 トランザクション）────────────────────
\* `SessionStore::append` の begin_write … commit。
\* 2 つのテーブルへの書き込みが 1 つの原子ステップで可視になる。

SafeCommit ==
    /\ writer_phase = "idle"
    /\ writer_phase' = "done"
    /\ disk_records' = 1
    /\ disk_meta' = 1
    /\ UNCHANGED reader_result

\* ── 読み手（いつでも割り込める。再起動もここに畳む）──────
\* bootstrap.rs の open_session_at_boot が読む側。

Read ==
    /\ reader_result = "none"
    /\ reader_result' = IF disk_records = disk_meta
                        THEN "consistent"
                        ELSE "torn"
    /\ UNCHANGED <<disk_records, disk_meta, writer_phase>>

NextBuggy ==
    \/ BuggyWriteRecord
    \/ BuggyWriteMeta
    \/ Read

SpecBuggy == Init /\ [][NextBuggy]_vars

NextSafe ==
    \/ SafeCommit
    \/ Read

SpecSafe == Init /\ [][NextSafe]_vars

\* ── 安全性 ────────────────────────────────────────────
\* 読み手は「レコードと meta の数が食い違う」状態を観測してはならない。
\* 破れたときの実害: list_sessions の件数が嘘をつき、RECORD_COUNT_WARN が
\* 出ない／二重に出る。seq そのものは RECORDS の最後 +1 から採るので
\* （`SessionStore::append` の seq 採番）、この破れだけでは上書きは起きない。
\* meta 由来に変えた版で何が起きるかは SessionSeqNoOverwrite で別に書く。

ReaderNeverSeesTornAppend ==
    reader_result # "torn"

====
