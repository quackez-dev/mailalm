//! Zone names: UTC's many spellings, and Windows' names (Microsoft Graph's, Outlook's invitations)
//! with the IANA zone each stands for.
//!
//! The table is the usual one (Unicode CLDR's `windowsZones`, the zone for territory "001"), for
//! the zones calendars commonly use. A Windows name it doesn't have is kept as it is, and its
//! times read as the provider gave them in UTC.

/// Windows' zone names and their IANA zones.
const WINDOWS: [(&str, &str); 78] = [
    ("Dateline Standard Time", "Etc/GMT+12"),
    ("UTC-11", "Etc/GMT+11"),
    ("Hawaiian Standard Time", "Pacific/Honolulu"),
    ("Alaskan Standard Time", "America/Anchorage"),
    ("Pacific Standard Time (Mexico)", "America/Tijuana"),
    ("Pacific Standard Time", "America/Los_Angeles"),
    ("US Mountain Standard Time", "America/Phoenix"),
    ("Mountain Standard Time (Mexico)", "America/Mazatlan"),
    ("Mountain Standard Time", "America/Denver"),
    ("Central America Standard Time", "America/Guatemala"),
    ("Central Standard Time (Mexico)", "America/Mexico_City"),
    ("Central Standard Time", "America/Chicago"),
    ("Canada Central Standard Time", "America/Regina"),
    ("SA Pacific Standard Time", "America/Bogota"),
    ("Eastern Standard Time (Mexico)", "America/Cancun"),
    ("Eastern Standard Time", "America/New_York"),
    ("US Eastern Standard Time", "America/Indiana/Indianapolis"),
    ("Venezuela Standard Time", "America/Caracas"),
    ("Paraguay Standard Time", "America/Asuncion"),
    ("Atlantic Standard Time", "America/Halifax"),
    ("SA Western Standard Time", "America/La_Paz"),
    ("Pacific SA Standard Time", "America/Santiago"),
    ("Newfoundland Standard Time", "America/St_Johns"),
    ("E. South America Standard Time", "America/Sao_Paulo"),
    ("Argentina Standard Time", "America/Argentina/Buenos_Aires"),
    ("SA Eastern Standard Time", "America/Cayenne"),
    ("Greenland Standard Time", "America/Nuuk"),
    ("Montevideo Standard Time", "America/Montevideo"),
    ("UTC-02", "Etc/GMT+2"),
    ("Azores Standard Time", "Atlantic/Azores"),
    ("Cape Verde Standard Time", "Atlantic/Cape_Verde"),
    ("UTC", "Etc/UTC"),
    ("GMT Standard Time", "Europe/London"),
    ("Greenwich Standard Time", "Atlantic/Reykjavik"),
    ("Morocco Standard Time", "Africa/Casablanca"),
    ("W. Europe Standard Time", "Europe/Berlin"),
    ("Central Europe Standard Time", "Europe/Budapest"),
    ("Romance Standard Time", "Europe/Paris"),
    ("Central European Standard Time", "Europe/Warsaw"),
    ("W. Central Africa Standard Time", "Africa/Lagos"),
    ("GTB Standard Time", "Europe/Bucharest"),
    ("E. Europe Standard Time", "Europe/Chisinau"),
    ("FLE Standard Time", "Europe/Kiev"),
    ("Egypt Standard Time", "Africa/Cairo"),
    ("South Africa Standard Time", "Africa/Johannesburg"),
    ("Israel Standard Time", "Asia/Jerusalem"),
    ("Turkey Standard Time", "Europe/Istanbul"),
    ("Jordan Standard Time", "Asia/Amman"),
    ("Middle East Standard Time", "Asia/Beirut"),
    ("Arabic Standard Time", "Asia/Baghdad"),
    ("Arab Standard Time", "Asia/Riyadh"),
    ("Russian Standard Time", "Europe/Moscow"),
    ("E. Africa Standard Time", "Africa/Nairobi"),
    ("Iran Standard Time", "Asia/Tehran"),
    ("Arabian Standard Time", "Asia/Dubai"),
    ("Afghanistan Standard Time", "Asia/Kabul"),
    ("Pakistan Standard Time", "Asia/Karachi"),
    ("India Standard Time", "Asia/Kolkata"),
    ("Sri Lanka Standard Time", "Asia/Colombo"),
    ("Nepal Standard Time", "Asia/Kathmandu"),
    ("Bangladesh Standard Time", "Asia/Dhaka"),
    ("SE Asia Standard Time", "Asia/Bangkok"),
    ("China Standard Time", "Asia/Shanghai"),
    ("Singapore Standard Time", "Asia/Singapore"),
    ("Taipei Standard Time", "Asia/Taipei"),
    ("W. Australia Standard Time", "Australia/Perth"),
    ("Tokyo Standard Time", "Asia/Tokyo"),
    ("Korea Standard Time", "Asia/Seoul"),
    ("Cen. Australia Standard Time", "Australia/Adelaide"),
    ("AUS Central Standard Time", "Australia/Darwin"),
    ("E. Australia Standard Time", "Australia/Brisbane"),
    ("AUS Eastern Standard Time", "Australia/Sydney"),
    ("Tasmania Standard Time", "Australia/Hobart"),
    ("New Zealand Standard Time", "Pacific/Auckland"),
    ("UTC+12", "Etc/GMT-12"),
    ("Fiji Standard Time", "Pacific/Fiji"),
    ("Tonga Standard Time", "Pacific/Tongatapu"),
    ("Samoa Standard Time", "Pacific/Apia"),
];

/// The IANA zone a Windows zone name stands for (any case).
pub fn iana_of_windows(name: &str) -> Option<&'static str> {
    let name = name.trim();
    WINDOWS.iter().find(|(w, _)| w.eq_ignore_ascii_case(name)).map(|(_, iana)| *iana)
}

/// IANA zones that follow the same rules as a Windows zone, though the table above names another
/// city for it (CLDR's territory-specific ones, for the places MailALM's users most likely are).
const ALIASES: [(&str, &str); 24] = [
    ("America/Toronto", "Eastern Standard Time"),
    ("America/Montreal", "Eastern Standard Time"),
    ("America/Detroit", "Eastern Standard Time"),
    ("America/Nassau", "Eastern Standard Time"),
    ("America/Vancouver", "Pacific Standard Time"),
    ("America/Edmonton", "Mountain Standard Time"),
    ("America/Winnipeg", "Central Standard Time"),
    ("America/Moncton", "Atlantic Standard Time"),
    ("America/Glace_Bay", "Atlantic Standard Time"),
    ("Europe/Dublin", "GMT Standard Time"),
    ("Europe/Lisbon", "GMT Standard Time"),
    ("Europe/Amsterdam", "W. Europe Standard Time"),
    ("Europe/Rome", "W. Europe Standard Time"),
    ("Europe/Vienna", "W. Europe Standard Time"),
    ("Europe/Zurich", "W. Europe Standard Time"),
    ("Europe/Stockholm", "W. Europe Standard Time"),
    ("Europe/Brussels", "Romance Standard Time"),
    ("Europe/Madrid", "Romance Standard Time"),
    ("Europe/Copenhagen", "Romance Standard Time"),
    ("Europe/Prague", "Central Europe Standard Time"),
    ("Europe/Helsinki", "FLE Standard Time"),
    ("Europe/Athens", "GTB Standard Time"),
    ("Asia/Hong_Kong", "China Standard Time"),
    ("Etc/UTC", "UTC"),
];

/// The Windows zone name for an IANA zone, when the tables have one.
pub fn windows_of_iana(iana: &str) -> Option<&'static str> {
    let iana = iana.trim();
    WINDOWS
        .iter()
        .find(|(_, z)| z.eq_ignore_ascii_case(iana))
        .map(|(w, _)| *w)
        .or_else(|| ALIASES.iter().find(|(z, _)| z.eq_ignore_ascii_case(iana)).map(|(_, w)| *w))
}

/// Whether a zone name is UTC (or GMT without summer time) under one of its spellings,
/// Microsoft Graph's included.
pub fn is_utc_name(name: &str) -> bool {
    let n = name.trim().to_ascii_lowercase();
    matches!(
        n.as_str(),
        "utc" | "etc/utc" | "etc/universal" | "universal" | "zulu" | "etc/zulu" | "z" | "gmt" | "etc/gmt" | "etc/gmt+0" | "etc/gmt-0" | "coordinated universal time" | "tzone://microsoft/utc"
    )
}
