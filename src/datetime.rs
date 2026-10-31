use windows::{
    Win32::{
        Foundation::{FILETIME, SYSTEMTIME},
        Globalization::{DATE_SHORTDATE, GetDateFormatEx, GetTimeFormatEx, TIME_NOSECONDS},
        System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTimeEx},
    },
    core::PCWSTR,
};

pub fn date(timestamp: u64) -> String {
    format(timestamp, false)
}
pub fn date_time(timestamp: u64) -> String {
    format(timestamp, true)
}

fn format(timestamp: u64, include_time: bool) -> String {
    let Some(ticks) = timestamp
        .checked_add(11_644_473_600)
        .and_then(|seconds| seconds.checked_mul(10_000_000))
    else {
        return "—".into();
    };
    let filetime = FILETIME {
        dwLowDateTime: ticks as u32,
        dwHighDateTime: (ticks >> 32) as u32,
    };
    let mut utc = SYSTEMTIME::default();
    let mut local = SYSTEMTIME::default();
    let mut date = [0; 128];
    let mut time = [0; 128];
    unsafe {
        if FileTimeToSystemTime(&filetime, &mut utc).is_err()
            || SystemTimeToTzSpecificLocalTimeEx(None, &utc, &mut local).is_err()
        {
            return "—".into();
        }
        let length = GetDateFormatEx(
            PCWSTR::null(),
            DATE_SHORTDATE,
            Some(&local),
            PCWSTR::null(),
            Some(&mut date),
            PCWSTR::null(),
        );
        if length <= 1 {
            return "—".into();
        }
        let date = String::from_utf16_lossy(&date[..length as usize - 1]);
        if !include_time {
            return date;
        }
        let length = GetTimeFormatEx(
            PCWSTR::null(),
            TIME_NOSECONDS,
            Some(&local),
            PCWSTR::null(),
            Some(&mut time),
        );
        if length <= 1 {
            return date;
        }
        format!(
            "{date} {}",
            String::from_utf16_lossy(&time[..length as usize - 1])
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn uses_windows_local_date_and_time_and_rejects_out_of_range_values() {
        let date = super::date(1_700_000_000);
        let datetime = super::date_time(1_700_000_000);
        assert_ne!(date, "—");
        assert!(datetime.starts_with(&date));
        assert!(datetime.len() > date.len());
        assert_eq!(super::date(u64::MAX), "—");
    }
}
