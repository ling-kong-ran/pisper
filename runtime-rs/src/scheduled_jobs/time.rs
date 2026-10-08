use crate::native_workflow::{Result, WorkflowError};
use chrono::{DateTime, Datelike, Duration, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use serde_json::Value;

fn zone(task: &Value) -> Result<Tz> {
    task["timezone"]
        .as_str()
        .unwrap_or("Asia/Hong_Kong")
        .parse::<Tz>()
        .map_err(|_| WorkflowError::coded("schedule_invalid_timezone", "定时任务时区无效。"))
}
fn month_days(year: i32, month: u32) -> Result<u32> {
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    }
    .ok_or_else(|| WorkflowError::invalid("定时任务日期无效。"))?;
    Ok((next - Duration::days(1)).day())
}
// 与 release 的 Intl 三次偏移迭代相同，包括 DST 缺失/重复墙钟时刻的处理。
fn wall_to_utc(date: NaiveDate, hour: u32, minute: u32, tz: Tz) -> Result<DateTime<Utc>> {
    let target = date
        .and_hms_opt(hour, minute, 0)
        .ok_or_else(|| WorkflowError::invalid("定时任务时间无效。"))?;
    let mut guess = Utc.from_utc_datetime(&target);
    for _ in 0..3 {
        let actual = guess.with_timezone(&tz).naive_local();
        guess -= actual - target;
    }
    Ok(guess)
}
pub(crate) fn next_run(task: &Value, from: DateTime<Utc>) -> Result<String> {
    if task["frequency"] == "interval" {
        let multiplier = match task["intervalUnit"].as_str().unwrap_or("hours") {
            "minutes" => 60000.0,
            "days" => 86400000.0,
            _ => 3600000.0,
        };
        let value = task["intervalValue"]
            .as_f64()
            .or_else(|| task["intervalValue"].as_str().and_then(|s| s.parse().ok()))
            .filter(|v| v.is_finite() && *v != 0.0)
            .unwrap_or(1.0)
            .clamp(1.0, 10000.0);
        let candidate = from + Duration::milliseconds((value * multiplier) as i64);
        return Ok(candidate.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    }
    let tz = zone(task)?;
    let time = task["time"].as_str().unwrap_or("09:00");
    let mut parts = time.split(':');
    let hour = parts
        .next()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(9);
    let minute = parts
        .next()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0);
    let local = from.with_timezone(&tz);
    let mut date = local.date_naive();
    let frequency = task["frequency"].as_str().unwrap_or("daily");
    if frequency == "weekly" {
        let weekday = date.weekday().num_days_from_sunday();
        let requested = task["dayOfWeek"].as_f64().unwrap_or(1.0).clamp(0.0, 6.0) as u32;
        date += Duration::days(((requested + 7 - weekday) % 7) as i64);
    } else if frequency == "monthly" {
        date = NaiveDate::from_ymd_opt(
            date.year(),
            date.month(),
            (task["dayOfMonth"].as_f64().unwrap_or(1.0).clamp(1.0, 31.0) as u32)
                .min(month_days(date.year(), date.month())?),
        )
        .ok_or_else(|| WorkflowError::invalid("定时任务日期无效。"))?;
    }
    let mut candidate = wall_to_utc(date, hour, minute, tz)?;
    if candidate <= from {
        date = match frequency {
            "daily" => date + Duration::days(1),
            "weekly" => date + Duration::days(7),
            _ => {
                let (year, month) = if date.month() == 12 {
                    (date.year() + 1, 1)
                } else {
                    (date.year(), date.month() + 1)
                };
                NaiveDate::from_ymd_opt(
                    year,
                    month,
                    (task["dayOfMonth"].as_f64().unwrap_or(1.0).clamp(1.0, 31.0) as u32)
                        .min(month_days(year, month)?),
                )
                .ok_or_else(|| WorkflowError::invalid("定时任务日期无效。"))?
            }
        };
        candidate = wall_to_utc(date, hour, minute, tz)?;
    }
    Ok(candidate.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
}
pub(crate) fn validate_timezone(task: &Value) -> Result<()> {
    zone(task).map(|_| ())
}
pub(crate) fn next_label(task: &Value) -> String {
    let Some(stamp) = task["nextRunAt"].as_str() else {
        return "未安排".into();
    };
    match (DateTime::parse_from_rfc3339(stamp), zone(task)) {
        (Ok(date), Ok(zone)) => date
            .with_timezone(&zone)
            .format("%Y年%-m月%-d日 %H:%M")
            .to_string(),
        _ => "未安排".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn next_time_matches_release_contract() {
        let from = "2026-07-18T10:00:00.000Z".parse::<DateTime<Utc>>().unwrap();
        let cases = [
            (
                json!({"frequency":"interval","intervalValue":30,"intervalUnit":"minutes"}),
                "2026-07-18T10:30:00.000Z",
            ),
            (
                json!({"frequency":"daily","time":"09:00","timezone":"UTC"}),
                "2026-07-19T09:00:00.000Z",
            ),
            (
                json!({"frequency":"weekly","dayOfWeek":0,"time":"09:00","timezone":"UTC"}),
                "2026-07-19T09:00:00.000Z",
            ),
            (
                json!({"frequency":"monthly","dayOfMonth":1,"time":"09:00","timezone":"UTC"}),
                "2026-08-01T09:00:00.000Z",
            ),
            (
                json!({"frequency":"daily","time":"09:00","timezone":"Asia/Hong_Kong"}),
                "2026-07-19T01:00:00.000Z",
            ),
        ];
        for (task, expected) in cases {
            assert_eq!(next_run(&task, from).unwrap(), expected);
        }
    }
    #[test]
    fn timezone_tracks_dst_instead_of_fixed_offset() {
        let task = json!({"frequency":"daily","time":"09:00","timezone":"America/New_York"});
        assert_eq!(
            next_run(&task, "2026-03-07T15:00:00Z".parse().unwrap()).unwrap(),
            "2026-03-08T13:00:00.000Z"
        );
        assert_eq!(
            next_run(&task, "2026-10-31T15:00:00Z".parse().unwrap()).unwrap(),
            "2026-11-01T14:00:00.000Z"
        );
        assert!(next_run(
            &json!({"frequency":"daily","timezone":"Mars/Nowhere"}),
            Utc::now()
        )
        .is_err());
    }
}
