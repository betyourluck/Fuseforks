//! 時刻で発火する依頼（Spec 07）の型と**発火規則**。
//!
//! ## このモジュールが純関数だけで出来ている理由
//!
//! 現在時刻は**必ず引数で受け取る**。内部で `Local::now()` を呼ばない。
//! 壁時計に依存したテストは「木曜の 17 時に走らせたときだけ落ちる」ような
//! 再現しない失敗を作るためで、Spec 04 で確立した規律をそのまま引く。
//!
//! タイムゾーンも型引数 `Tz` で受ける。実行機の設定（日本なら JST）に
//! テストが左右されなくなるうえ、**DST を持つタイムゾーンを渡せる**ので、
//! 「存在しない時刻・曖昧な時刻で panic しない」契約を実際に走らせて確かめられる。
//!
//! ## 消化（consume）という語
//!
//! [`ScheduledTask::last_consumed_due_ms`] は「**消化した予定時刻**」であって
//! 発火時刻ではない。飛ばした予定も消化済みにするので、この 1 つの欄で
//! 「発火した」と「発火せず飛ばした」の両方を表せる。発火時刻を持つ設計にすると、
//! 飛ばしたことを覚える場所が別に要る。

use chrono::{DateTime, Datelike, Days, NaiveDateTime, NaiveTime, TimeDelta, TimeZone};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::model::AgentId;

/// 壁時計系の予定に許す遅れ（分）。
///
/// これを過ぎた予定は**発火せずに消化済みへ倒す**。17 時の鐘を 23 時に鳴らすのは
/// 「鳴らなかった」より悪い。
///
/// **間隔系には適用しない。** 間隔の予定に「間違った時刻」は存在しないので、
/// 猶予を掛ける理由が無い（掛けると、長く止まっていた予定が毎 tick 消化される
/// だけで一度も発火しない）。
///
/// 5 分という値の根拠は薄い — tick 間隔より十分長く、「うっかり閉じていた」を
/// 拾わない程度に短い、以外に無い。実測してから決め直す余地がある。
pub const GRACE_MINUTES: i64 = 5;

/// 曜日。`chrono::Weekday` を直接持たないのは、保存形式（`schedules.json`）の
/// 表記をこちら側の契約として固定するため。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Weekday {
    /// 月曜。
    Mon,
    /// 火曜。
    Tue,
    /// 水曜。
    Wed,
    /// 木曜。
    Thu,
    /// 金曜。
    Fri,
    /// 土曜。
    Sat,
    /// 日曜。
    Sun,
}

impl Weekday {
    /// `chrono` 側の表現へ移す。
    fn to_chrono(self) -> chrono::Weekday {
        match self {
            Self::Mon => chrono::Weekday::Mon,
            Self::Tue => chrono::Weekday::Tue,
            Self::Wed => chrono::Weekday::Wed,
            Self::Thu => chrono::Weekday::Thu,
            Self::Fri => chrono::Weekday::Fri,
            Self::Sat => chrono::Weekday::Sat,
            Self::Sun => chrono::Weekday::Sun,
        }
    }

    /// 画面と発話に出す日本語表記（「木曜」）。
    pub fn label_ja(self) -> &'static str {
        match self {
            Self::Mon => "月曜",
            Self::Tue => "火曜",
            Self::Wed => "水曜",
            Self::Thu => "木曜",
            Self::Fri => "金曜",
            Self::Sat => "土曜",
            Self::Sun => "日曜",
        }
    }
}

/// 再現規則。
///
/// **cron 式は採らない。** `0 17 * * 4` は表現力が高い代わりに読めない人には
/// 一切読めず、UI も自由入力欄にしかならない。要望の 2 例（毎週 X 曜 hh:mm /
/// 定期的に）が言い切れる最小の構造を採る。後から cron 式を足すことはできるが、
/// 外すことはできない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Recurrence {
    /// n 分ごと。
    #[serde(rename_all = "camelCase")]
    Interval {
        /// 間隔（分）。1 以上。
        every_minutes: u32,
    },
    /// 毎日 hh:mm（現地時刻）。
    #[serde(rename_all = "camelCase")]
    Daily {
        /// 時（0〜23）。
        hour: u8,
        /// 分（0〜59）。
        minute: u8,
    },
    /// 毎週 X 曜 hh:mm（現地時刻）。
    #[serde(rename_all = "camelCase")]
    Weekly {
        /// 曜日。
        weekday: Weekday,
        /// 時（0〜23）。
        hour: u8,
        /// 分（0〜59）。
        minute: u8,
    },
}

/// 再現規則として受け付けられない値。
///
/// UI で防ぐだけでは `schedules.json` を手で編集された時に入るので、
/// **読み込みでも弾く**（Spec 05 で `world.json` の直接編集が作った穴と同じ形）。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InvalidRecurrence {
    /// `everyMinutes` が 0。
    #[error("everyMinutes は 1 以上である必要があります（受け取った値: {0}）")]
    ZeroInterval(u32),
    /// `hour` が 24 以上。
    #[error("hour は 0〜23 である必要があります（受け取った値: {0}）")]
    HourOutOfRange(u8),
    /// `minute` が 60 以上。
    #[error("minute は 0〜59 である必要があります（受け取った値: {0}）")]
    MinuteOutOfRange(u8),
}

impl Recurrence {
    /// 値が規則として成立するかを検査する。
    pub fn validate(&self) -> Result<(), InvalidRecurrence> {
        match self {
            Self::Interval { every_minutes } => {
                if *every_minutes == 0 {
                    return Err(InvalidRecurrence::ZeroInterval(*every_minutes));
                }
            }
            Self::Daily { hour, minute } | Self::Weekly { hour, minute, .. } => {
                if *hour > 23 {
                    return Err(InvalidRecurrence::HourOutOfRange(*hour));
                }
                if *minute > 59 {
                    return Err(InvalidRecurrence::MinuteOutOfRange(*minute));
                }
            }
        }
        Ok(())
    }

    /// 時刻に意味がある規則（daily / weekly）かどうか。
    ///
    /// **猶予を掛けてよいのはこちらだけ**、という判断がこの述語 1 つに集まっている。
    pub fn is_wall_clock(&self) -> bool {
        matches!(self, Self::Daily { .. } | Self::Weekly { .. })
    }

    /// 画面と発話に出す日本語表記（「毎週 木曜 17:00」「10 分ごと」）。
    ///
    /// 配送本文の先頭に付ける `【定期実行: …】` と UI の一覧で**同じ関数を使う**。
    /// 別々に組み立てると、1 つのものに真実が 2 箇所できる。
    pub fn label_ja(&self) -> String {
        match self {
            Self::Interval { every_minutes } => format!("{every_minutes} 分ごと"),
            Self::Daily { hour, minute } => format!("毎日 {hour:02}:{minute:02}"),
            Self::Weekly {
                weekday,
                hour,
                minute,
            } => format!("毎週 {} {hour:02}:{minute:02}", weekday.label_ja()),
        }
    }

    /// この規則の時刻部分を [`NaiveTime`] として取り出す（壁時計系のみ）。
    fn naive_time(&self) -> Option<NaiveTime> {
        match self {
            Self::Interval { .. } => None,
            Self::Daily { hour, minute } | Self::Weekly { hour, minute, .. } => {
                NaiveTime::from_hms_opt(u32::from(*hour), u32::from(*minute), 0)
            }
        }
    }
}

/// 現地時刻へ変換する。**`unwrap` しない。**
///
/// [`TimeZone::from_local_datetime`] は DST の飛ぶ時刻（春の 2:30）で `None`、
/// 戻る時刻で `Ambiguous` を返す。[`chrono::LocalResult::single`] は
/// **どちらも `None`** に畳むので、その回は発火しない扱いになる。
/// 日本に DST は無いが、panic する経路を他のタイムゾーンの利用者に残さない。
fn resolve_local<Tz: TimeZone>(reference: &DateTime<Tz>, naive: NaiveDateTime) -> Option<DateTime<Tz>> {
    reference.timezone().from_local_datetime(&naive).single()
}

/// 壁時計系（daily / weekly）の `due` — **`now` 以前にある直近の予定時刻**。
///
/// 間隔系を渡された場合は `None`（呼び分けを間違えても発火しない側へ倒す）。
pub fn due_wall_clock<Tz: TimeZone>(
    recurrence: &Recurrence,
    now: &DateTime<Tz>,
) -> Option<DateTime<Tz>> {
    let time = recurrence.naive_time()?;

    let base_date = match recurrence {
        Recurrence::Interval { .. } => return None,
        Recurrence::Daily { .. } => now.date_naive(),
        Recurrence::Weekly { weekday, .. } => {
            // 直近の（今日を含む）当該曜日まで戻る。
            let target = i64::from(weekday.to_chrono().num_days_from_monday());
            let current = i64::from(now.weekday().num_days_from_monday());
            let back = (current - target).rem_euclid(7);
            now.date_naive().checked_sub_days(Days::new(back as u64))?
        }
    };

    // 1 周期ぶん戻る幅。daily は 1 日、weekly は 7 日。
    let step = match recurrence {
        Recurrence::Weekly { .. } => 7,
        _ => 1,
    };

    let candidate = resolve_local(now, base_date.and_time(time))?;
    if candidate <= *now {
        return Some(candidate);
    }

    // 今日（今週）の予定時刻はまだ来ていない。1 周期前を見る。
    let previous = base_date.checked_sub_days(Days::new(step))?;
    let candidate = resolve_local(now, previous.and_time(time))?;
    (candidate <= *now).then_some(candidate)
}

/// 間隔系の `due` — 起点から刻んで **`now` 以前の最後の点**。
///
/// 「最後の点」であることが「再開時に 1 回だけ」を成立させる。2 日止まっていた
/// 10 分ごとの予定は、288 個の点を全部撒くのでも 0 個でもなく、`now` の直前に
/// 落ちる 1 点だけを返す。
///
/// `every_minutes == 0` は [`Recurrence::validate`] が弾く値だが、ここでも
/// `None` を返す。ゼロ除算で panic する経路を残さない。
pub fn due_interval<Tz: TimeZone>(
    anchor: &DateTime<Tz>,
    every_minutes: u32,
    now: &DateTime<Tz>,
) -> Option<DateTime<Tz>> {
    if every_minutes == 0 {
        return None;
    }
    let every = i64::from(every_minutes);

    let elapsed = now.clone().signed_duration_since(anchor.clone());
    if elapsed < TimeDelta::minutes(every) {
        return None;
    }

    let k = elapsed.num_minutes() / every; // floor
    anchor.clone().checked_add_signed(TimeDelta::try_minutes(every.checked_mul(k)?)?)
}

/// 1 回の tick で予定に対して取るべき行動。
///
/// `due_ms` は**消化する予定時刻**であって、いま現在の時刻ではない。
/// これをそのまま [`ScheduledTask::last_consumed_due_ms`] へ書くことで刻みの位相が保たれる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tick {
    /// 何もしない。
    Idle,
    /// 発火せずに消化済みにする（猶予を過ぎた壁時計系）。
    Consume {
        /// 消化する予定時刻（epoch ミリ秒）。
        due_ms: u64,
    },
    /// 発火し、消化済みにする。
    Fire {
        /// 消化する予定時刻（epoch ミリ秒）。
        due_ms: u64,
    },
}

/// 時刻で発火する依頼 1 件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledTask {
    /// 一意 ID（UUID v4）。
    pub id: String,
    /// 宛先。自分宛でもワーカー宛でも区別しない。
    pub to: AgentId,
    /// 届ける依頼の本文。
    pub message: String,
    /// 再現規則。
    pub recurrence: Recurrence,
    /// 作成時刻（epoch ミリ秒）。間隔系の起点になる。
    ///
    /// [`Self::last_consumed_due_ms`] が `None` のとき、ここから刻みが始まる。
    /// この欄が無いと、再起動後に間隔系の起点が計算できなくなる。
    pub created_at_ms: u64,
    /// 直近に**消化した**予定時刻（epoch ミリ秒）。発火時刻ではない。
    pub last_consumed_due_ms: Option<u64>,
    /// 偽なら発火も消化もしない。既定は真。
    ///
    /// 消化もしないので、再開したときは**その時点の直近の予定**から拾える。
    #[serde(default = "enabled_default")]
    pub enabled: bool,
}

/// `enabled` の既定値（真）。
fn enabled_default() -> bool {
    true
}

impl ScheduledTask {
    /// 間隔系の起点（消化済みがあればそれ、無ければ作成時刻）。
    pub fn anchor_ms(&self) -> u64 {
        self.last_consumed_due_ms.unwrap_or(self.created_at_ms)
    }

    /// `now` 以前にある直近の予定時刻。
    pub fn due<Tz: TimeZone>(&self, now: &DateTime<Tz>) -> Option<DateTime<Tz>> {
        match self.recurrence {
            Recurrence::Interval { every_minutes } => {
                let anchor = millis_to_datetime(now, self.anchor_ms())?;
                due_interval(&anchor, every_minutes, now)
            }
            _ => due_wall_clock(&self.recurrence, now),
        }
    }

    /// UI の「次回の発火予定時刻」— `now` より**後**の最初の予定時刻。
    ///
    /// `enabled` が偽でも時刻そのものは返す（止まっていることの表示は UI 側の仕事）。
    pub fn next_due<Tz: TimeZone>(&self, now: &DateTime<Tz>) -> Option<DateTime<Tz>> {
        match self.recurrence {
            Recurrence::Interval { every_minutes } => {
                if every_minutes == 0 {
                    return None;
                }
                let every = i64::from(every_minutes);
                let anchor = millis_to_datetime(now, self.anchor_ms())?;
                let elapsed = now.clone().signed_duration_since(anchor.clone());
                // 起点がまだ未来なら最初の点は anchor + every。
                let steps = (elapsed.num_minutes() / every).max(0) + 1;
                anchor.checked_add_signed(TimeDelta::try_minutes(every.checked_mul(steps)?)?)
            }
            _ => {
                let time = self.recurrence.naive_time()?;
                let step = if matches!(self.recurrence, Recurrence::Weekly { .. }) {
                    7
                } else {
                    1
                };
                // 直近の過去の点から 1 周期進めれば、必ず now より後の最初の点になる。
                match due_wall_clock(&self.recurrence, now) {
                    Some(previous) => {
                        let date = previous.date_naive().checked_add_days(Days::new(step))?;
                        resolve_local(now, date.and_time(time))
                    }
                    // まだ一度も来ていない = 今日（今週）の点が未来にある。
                    None => {
                        let base = match self.recurrence {
                            Recurrence::Weekly { weekday, .. } => {
                                let target =
                                    i64::from(weekday.to_chrono().num_days_from_monday());
                                let current = i64::from(now.weekday().num_days_from_monday());
                                let ahead = (target - current).rem_euclid(7);
                                now.date_naive().checked_add_days(Days::new(ahead as u64))?
                            }
                            _ => now.date_naive(),
                        };
                        resolve_local(now, base.and_time(time))
                    }
                }
            }
        }
    }

    /// この tick で取るべき行動を決める。**この関数が発火規則そのもの。**
    ///
    /// | 条件 | 結果 |
    /// |---|---|
    /// | `enabled == false` | [`Tick::Idle`]（消化もしない） |
    /// | `due` が無い | [`Tick::Idle`] |
    /// | 消化済み | [`Tick::Idle`] |
    /// | 壁時計系で猶予超過 | [`Tick::Consume`] |
    /// | それ以外 | [`Tick::Fire`] |
    pub fn decide<Tz: TimeZone>(&self, now: &DateTime<Tz>) -> Tick {
        if !self.enabled {
            return Tick::Idle;
        }

        let Some(due) = self.due(now) else {
            return Tick::Idle;
        };

        // epoch 前（1970 年より過去）の予定時刻は作成時刻が壊れている場合しか
        // 生まれない。u64 へ落とせないので発火しない側へ倒す。
        let Ok(due_ms) = u64::try_from(due.timestamp_millis()) else {
            return Tick::Idle;
        };

        if self.last_consumed_due_ms.is_some_and(|last| last >= due_ms) {
            return Tick::Idle;
        }

        if self.recurrence.is_wall_clock() {
            let late = now.clone().signed_duration_since(due);
            if late > TimeDelta::minutes(GRACE_MINUTES) {
                return Tick::Consume { due_ms };
            }
        }

        Tick::Fire { due_ms }
    }
}

/// epoch ミリ秒を、`reference` と同じタイムゾーンの日時へ移す。
fn millis_to_datetime<Tz: TimeZone>(reference: &DateTime<Tz>, ms: u64) -> Option<DateTime<Tz>> {
    let ms = i64::try_from(ms).ok()?;
    reference.timezone().timestamp_millis_opt(ms).single()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    /// 実行機の設定に左右されないよう、テストは常に固定オフセットで組む。
    fn jst() -> FixedOffset {
        FixedOffset::east_opt(9 * 3600).expect("JST は妥当なオフセット")
    }

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> DateTime<FixedOffset> {
        jst()
            .with_ymd_and_hms(y, m, d, h, min, s)
            .single()
            .expect("テストの時刻は一意に決まる")
    }

    fn task(recurrence: Recurrence, created: DateTime<FixedOffset>) -> ScheduledTask {
        ScheduledTask {
            id: "task_01".to_owned(),
            to: AgentId::from("agent_01"),
            message: "今の時刻を言って".to_owned(),
            recurrence,
            created_at_ms: created.timestamp_millis() as u64,
            last_consumed_due_ms: None,
            enabled: true,
        }
    }

    const THU_17: Recurrence = Recurrence::Weekly {
        weekday: Weekday::Thu,
        hour: 17,
        minute: 0,
    };

    // 2026-07-30 は木曜。

    #[test]
    fn weekly_fires_once_when_crossing_due() {
        let created = at(2026, 7, 27, 10, 0, 0); // 月曜
        let t = task(THU_17, created);

        // 17:00:29 — tick 30 秒なので、跨いだ直後はこう見える。
        let now = at(2026, 7, 30, 17, 0, 29);
        let due = at(2026, 7, 30, 17, 0, 0);
        assert_eq!(
            t.decide(&now),
            Tick::Fire {
                due_ms: due.timestamp_millis() as u64
            }
        );
    }

    #[test]
    fn weekly_does_not_fire_twice_in_the_same_week() {
        let created = at(2026, 7, 27, 10, 0, 0);
        let mut t = task(THU_17, created);
        let due = at(2026, 7, 30, 17, 0, 0);

        t.last_consumed_due_ms = Some(due.timestamp_millis() as u64);

        for now in [
            at(2026, 7, 30, 17, 0, 59),
            at(2026, 7, 30, 23, 59, 0),
            at(2026, 8, 2, 12, 0, 0),
        ] {
            assert_eq!(t.decide(&now), Tick::Idle, "{now} で再発火した");
        }
    }

    #[test]
    fn weekly_fires_again_next_week() {
        let created = at(2026, 7, 27, 10, 0, 0);
        let mut t = task(THU_17, created);
        t.last_consumed_due_ms = Some(at(2026, 7, 30, 17, 0, 0).timestamp_millis() as u64);

        let now = at(2026, 8, 6, 17, 0, 10);
        let due = at(2026, 8, 6, 17, 0, 0);
        assert_eq!(
            t.decide(&now),
            Tick::Fire {
                due_ms: due.timestamp_millis() as u64
            }
        );
    }

    #[test]
    fn daily_missed_by_six_hours_is_consumed_without_firing() {
        let created = at(2026, 7, 29, 8, 0, 0);
        let t = task(Recurrence::Daily { hour: 9, minute: 0 }, created);

        let now = at(2026, 7, 30, 15, 0, 0); // 6 時間後に起動した
        let due = at(2026, 7, 30, 9, 0, 0);
        assert_eq!(
            t.decide(&now),
            Tick::Consume {
                due_ms: due.timestamp_millis() as u64
            }
        );
    }

    #[test]
    fn daily_consumed_task_is_not_re_evaluated_that_day() {
        let created = at(2026, 7, 29, 8, 0, 0);
        let mut t = task(Recurrence::Daily { hour: 9, minute: 0 }, created);
        t.last_consumed_due_ms = Some(at(2026, 7, 30, 9, 0, 0).timestamp_millis() as u64);

        assert_eq!(t.decide(&at(2026, 7, 30, 15, 0, 30)), Tick::Idle);
        assert_eq!(t.decide(&at(2026, 7, 30, 23, 59, 0)), Tick::Idle);
    }

    #[test]
    fn interval_stopped_two_days_fires_exactly_once() {
        let created = at(2026, 7, 28, 9, 0, 0);
        let t = task(Recurrence::Interval { every_minutes: 10 }, created);

        // 2 日ぶりに起動した。rev1 の規則ではここが 0 回だった。
        let now = at(2026, 7, 30, 9, 3, 0);
        let Tick::Fire { due_ms } = t.decide(&now) else {
            panic!("再開直後に発火しなかった: {:?}", t.decide(&now));
        };

        // 消化したあとは、次の 10 分が来るまで発火しない（288 回撒かない）。
        let mut after = t.clone();
        after.last_consumed_due_ms = Some(due_ms);
        assert_eq!(after.decide(&now), Tick::Idle);
        assert_eq!(after.decide(&at(2026, 7, 30, 9, 9, 0)), Tick::Idle);
        assert!(matches!(
            after.decide(&at(2026, 7, 30, 9, 13, 0)),
            Tick::Fire { .. }
        ));
    }

    /// rev1 のバグの回帰テスト。
    ///
    /// `due` を最遅点にする修正だけでは、`everyMinutes > GRACE` の予定が
    /// 依然 skip される（60 分ごとで 2900 分停止すると `k=48`、
    /// `due = anchor + 2880`、`now - due = 20 分 > 猶予 5 分`）。
    /// **猶予を間隔系へ掛けない**修正が揃って初めて発火する。
    #[test]
    fn interval_longer_than_grace_still_fires_after_long_stop() {
        let created = at(2026, 7, 28, 0, 0, 0);
        let t = task(Recurrence::Interval { every_minutes: 60 }, created);

        // 2900 分後 = 2 日 + 20 分。
        let now = created + TimeDelta::minutes(2900);
        let expected = created + TimeDelta::minutes(2880);

        assert_eq!(
            t.decide(&now),
            Tick::Fire {
                due_ms: expected.timestamp_millis() as u64
            },
            "猶予を間隔系へ掛けると、ここが Idle になる"
        );
    }

    #[test]
    fn interval_does_not_fire_before_the_first_step() {
        let created = at(2026, 7, 30, 9, 0, 0);
        let t = task(Recurrence::Interval { every_minutes: 10 }, created);

        assert_eq!(t.decide(&at(2026, 7, 30, 9, 0, 30)), Tick::Idle);
        assert_eq!(t.decide(&at(2026, 7, 30, 9, 9, 59)), Tick::Idle);
        assert!(matches!(
            t.decide(&at(2026, 7, 30, 9, 10, 0)),
            Tick::Fire { .. }
        ));
    }

    #[test]
    fn disabled_task_neither_fires_nor_consumes() {
        let created = at(2026, 7, 27, 10, 0, 0);
        let mut t = task(THU_17, created);
        t.enabled = false;

        // 猶予をとうに過ぎた時刻でも、消化しない。
        let now = at(2026, 7, 30, 23, 0, 0);
        assert_eq!(t.decide(&now), Tick::Idle);
        assert_eq!(
            t.last_consumed_due_ms, None,
            "decide は純関数なので状態を触らない"
        );

        // 再開すれば、その時点の直近の予定から拾える。
        t.enabled = true;
        assert!(matches!(
            t.decide(&at(2026, 8, 6, 17, 0, 5)),
            Tick::Fire { .. }
        ));
    }

    #[test]
    fn validation_rejects_out_of_range_values() {
        assert_eq!(
            Recurrence::Daily {
                hour: 99,
                minute: 0
            }
            .validate(),
            Err(InvalidRecurrence::HourOutOfRange(99))
        );
        assert_eq!(
            Recurrence::Weekly {
                weekday: Weekday::Thu,
                hour: 17,
                minute: 60
            }
            .validate(),
            Err(InvalidRecurrence::MinuteOutOfRange(60))
        );
        assert_eq!(
            Recurrence::Interval { every_minutes: 0 }.validate(),
            Err(InvalidRecurrence::ZeroInterval(0))
        );
        assert!(THU_17.validate().is_ok());
    }

    /// 検証で弾く値だが、規則側もゼロ除算で panic しない。
    #[test]
    fn zero_interval_never_panics() {
        let created = at(2026, 7, 30, 9, 0, 0);
        let t = task(Recurrence::Interval { every_minutes: 0 }, created);
        assert_eq!(t.decide(&at(2026, 7, 30, 12, 0, 0)), Tick::Idle);
        assert_eq!(t.next_due(&at(2026, 7, 30, 12, 0, 0)), None);
    }

    #[test]
    fn next_due_points_after_now() {
        let created = at(2026, 7, 27, 10, 0, 0);

        let weekly = task(THU_17, created);
        assert_eq!(
            weekly.next_due(&at(2026, 7, 30, 17, 0, 30)),
            Some(at(2026, 8, 6, 17, 0, 0)),
            "跨いだ直後は翌週を指す"
        );
        assert_eq!(
            weekly.next_due(&at(2026, 7, 30, 9, 0, 0)),
            Some(at(2026, 7, 30, 17, 0, 0)),
            "当日の予定時刻が未来ならその日を指す"
        );
        assert_eq!(
            weekly.next_due(&at(2026, 7, 31, 9, 0, 0)),
            Some(at(2026, 8, 6, 17, 0, 0)),
            "過ぎた翌日は翌週を指す"
        );

        let daily = task(Recurrence::Daily { hour: 9, minute: 0 }, created);
        assert_eq!(
            daily.next_due(&at(2026, 7, 30, 15, 0, 0)),
            Some(at(2026, 7, 31, 9, 0, 0))
        );
        assert_eq!(
            daily.next_due(&at(2026, 7, 30, 8, 0, 0)),
            Some(at(2026, 7, 30, 9, 0, 0))
        );

        let interval = task(
            Recurrence::Interval { every_minutes: 10 },
            at(2026, 7, 30, 9, 0, 0),
        );
        assert_eq!(
            interval.next_due(&at(2026, 7, 30, 9, 3, 0)),
            Some(at(2026, 7, 30, 9, 10, 0))
        );
        assert_eq!(
            interval.next_due(&at(2026, 7, 30, 9, 25, 0)),
            Some(at(2026, 7, 30, 9, 30, 0))
        );
    }

    #[test]
    fn labels_are_readable() {
        assert_eq!(THU_17.label_ja(), "毎週 木曜 17:00");
        assert_eq!(
            Recurrence::Daily { hour: 9, minute: 5 }.label_ja(),
            "毎日 09:05"
        );
        assert_eq!(
            Recurrence::Interval { every_minutes: 10 }.label_ja(),
            "10 分ごと"
        );
    }

    #[test]
    fn json_shape_matches_the_contract() {
        let created = at(2026, 7, 30, 9, 0, 0);
        let t = task(THU_17, created);
        let json = serde_json::to_value(&t).expect("直列化できる");

        assert_eq!(json["recurrence"]["kind"], "weekly");
        assert_eq!(json["recurrence"]["weekday"], "thu");
        assert!(json["createdAtMs"].is_u64());
        assert!(json["lastConsumedDueMs"].is_null());

        let interval = Recurrence::Interval { every_minutes: 10 };
        let json = serde_json::to_value(interval).expect("直列化できる");
        assert_eq!(json["kind"], "interval");
        assert_eq!(json["everyMinutes"], 10);
    }

    /// `enabled` を持たない古い（または手書きの）JSON は真として読む。
    #[test]
    fn enabled_defaults_to_true_when_absent() {
        let json = r#"{
            "id": "task_01",
            "to": "agent_01",
            "message": "点検",
            "recurrence": { "kind": "daily", "hour": 9, "minute": 0 },
            "createdAtMs": 1700000000000,
            "lastConsumedDueMs": null
        }"#;
        let t: ScheduledTask = serde_json::from_str(json).expect("読める");
        assert!(t.enabled);
    }

    /// DST を持つタイムゾーンでの挙動。**日本では一度も通れない経路**なので、
    /// テストだけ `chrono-tz` を使って実際に走らせる。
    mod dst {
        use super::*;
        use chrono_tz::America::New_York;

        /// 春の飛ぶ時刻（2026-03-08 02:30 は存在しない）。
        #[test]
        fn nonexistent_local_time_does_not_fire_and_does_not_panic() {
            let now = New_York
                .with_ymd_and_hms(2026, 3, 8, 5, 0, 0)
                .single()
                .expect("この時刻は一意");
            let t = ScheduledTask {
                id: "task_dst".to_owned(),
                to: AgentId::from("agent_01"),
                message: "点検".to_owned(),
                recurrence: Recurrence::Daily {
                    hour: 2,
                    minute: 30,
                },
                created_at_ms: 0,
                last_consumed_due_ms: None,
                enabled: true,
            };

            // 02:30 は存在しないので、この日は発火も消化もしない。
            assert_eq!(t.decide(&now), Tick::Idle);
        }

        /// 秋の戻る時刻（2026-11-01 01:30 は 2 回ある）。
        #[test]
        fn ambiguous_local_time_does_not_fire_and_does_not_panic() {
            let now = New_York
                .with_ymd_and_hms(2026, 11, 1, 5, 0, 0)
                .single()
                .expect("この時刻は一意");
            let t = ScheduledTask {
                id: "task_dst".to_owned(),
                to: AgentId::from("agent_01"),
                message: "点検".to_owned(),
                recurrence: Recurrence::Daily {
                    hour: 1,
                    minute: 30,
                },
                created_at_ms: 0,
                last_consumed_due_ms: None,
                enabled: true,
            };

            assert_eq!(t.decide(&now), Tick::Idle);
        }

        /// DST を跨いでも間隔系は素直に刻む（絶対時間で計算しているため）。
        #[test]
        fn interval_crosses_dst_without_losing_a_step() {
            let anchor = New_York
                .with_ymd_and_hms(2026, 3, 8, 0, 30, 0)
                .single()
                .expect("この時刻は一意");
            let now = anchor + TimeDelta::minutes(125);

            let due = due_interval(&anchor, 60, &now).expect("刻みが取れる");
            assert_eq!(due, anchor + TimeDelta::minutes(120));
        }
    }
}
