//! The sample mailbox shown until accounts can be connected (phase 2 and later). Every name and
//! address here is made up.

use mailalm_model::{Account, AccountId, ImapPreset, Provider, Rgb, Workspace};

pub const OUTLOOK: AccountId = AccountId(1);
pub const GMAIL: AccountId = AccountId(2);
pub const VIDEOTRON: AccountId = AccountId(3);
pub const BELL: AccountId = AccountId(4);

pub fn workspace(one_account: bool) -> Workspace {
    let mut accounts = vec![
        Account { id: OUTLOOK, name: "Outlook".into(), address: "alex.morin@hotmail.com".into(), provider: Provider::Outlook, color: Rgb(0x5b, 0x9b, 0xff) },
        Account { id: GMAIL, name: "Gmail".into(), address: "alex.morin@gmail.com".into(), provider: Provider::Gmail, color: Rgb(0xf2, 0x91, 0x4f) },
        Account {
            id: VIDEOTRON,
            name: "Vidéotron".into(),
            address: "amorin@videotron.ca".into(),
            provider: Provider::Imap(ImapPreset::Videotron),
            color: Rgb(0xb8, 0x93, 0xff),
        },
        Account { id: BELL, name: "Bell".into(), address: "alex.morin@bell.net".into(), provider: Provider::Imap(ImapPreset::Bell), color: Rgb(0xe3, 0xb5, 0x3f) },
    ];
    if one_account {
        accounts.truncate(1);
    }
    Workspace::new(accounts)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Group {
    Today,
    Yesterday,
    LastWeek,
}

impl Group {
    pub const ALL: [Group; 3] = [Group::Today, Group::Yesterday, Group::LastWeek];

    pub fn label(self) -> &'static str {
        match self {
            Group::Today => "TODAY",
            Group::Yesterday => "YESTERDAY",
            Group::LastWeek => "LAST WEEK",
        }
    }
}

pub struct Attachment {
    pub name: &'static str,
    pub kind: &'static str,
    pub size: &'static str,
}

pub struct Message {
    pub account: AccountId,
    pub from: &'static str,
    pub address: &'static str,
    pub initials: &'static str,
    pub avatar: egui::Color32,
    pub subject: &'static str,
    pub preview: &'static str,
    pub body: &'static str,
    pub time: &'static str,
    pub group: Group,
    pub unread: bool,
    pub flagged: bool,
    pub chip: Option<&'static str>,
    pub attachments: Vec<Attachment>,
}

fn rgb(r: u8, g: u8, b: u8) -> egui::Color32 {
    egui::Color32::from_rgb(r, g, b)
}

pub fn messages() -> Vec<Message> {
    vec![
        Message {
            account: OUTLOOK,
            from: "Zoe Park",
            address: "zoe@studio.example",
            initials: "ZP",
            avatar: rgb(0x6b, 0x5b, 0xd6),
            subject: "Storyboard review on Thursday",
            preview: "Hi Alex, the animatic is in good shape. Could you bring the revised frames…",
            body: "Hi Alex,\n\nThe animatic is in good shape. Could you bring the revised frames for scenes 4 to 7 on Thursday? I attached the colour key we agreed on, and the shot list as a PDF, so we stay consistent.\n\nThanks,\nZoe",
            time: "10:34",
            group: Group::Today,
            unread: false,
            flagged: true,
            chip: None,
            attachments: vec![
                Attachment { name: "colour-key.png", kind: "PNG image", size: "268 KB" },
                Attachment { name: "shot-list.pdf", kind: "PDF document", size: "1.2 MB" },
            ],
        },
        Message {
            account: GMAIL,
            from: "Camille Roy",
            address: "camille@collective.example",
            initials: "CR",
            avatar: rgb(0x2f, 0x6f, 0x8f),
            subject: "Invitation: Budget review",
            preview: "Thu Nov 26, 14:00–15:00 · Google Meet",
            body: "Hi all,\n\nLet's go over the Q1 budget: festival costs, printing and the sound studio. One hour should be enough.",
            time: "09:58",
            group: Group::Today,
            unread: true,
            flagged: false,
            chip: Some("Invitation · not in calendar yet"),
            attachments: Vec::new(),
        },
        Message {
            account: BELL,
            from: "Print Shop",
            address: "orders@printshop.example",
            initials: "PS",
            avatar: rgb(0xa2, 0x48, 0x2c),
            subject: "Your proof for order 4471 is ready",
            preview: "Hello, the proof for the festival poster (A2, matte) is ready for review…",
            body: "Hello,\n\nThe proof for the festival poster (A2, matte) is ready for review. Please confirm before Wednesday so we can print on Thursday.",
            time: "09:12",
            group: Group::Today,
            unread: true,
            flagged: false,
            chip: None,
            attachments: vec![Attachment { name: "proof-4471.pdf", kind: "PDF document", size: "3.4 MB" }],
        },
        Message {
            account: OUTLOOK,
            from: "Sam Okafor",
            address: "sam@studio.example",
            initials: "SO",
            avatar: rgb(0x3f, 0x7d, 0x3a),
            subject: "Re: Screening room",
            preview: "Works for me. I will book the small room from 2 pm.",
            body: "Works for me. I will book the small room from 2 pm.",
            time: "08:47",
            group: Group::Today,
            unread: true,
            flagged: false,
            chip: Some("Back from snooze"),
            attachments: Vec::new(),
        },
        Message {
            account: GMAIL,
            from: "Ana Lima",
            address: "ana@sound.example",
            initials: "AL",
            avatar: rgb(0x8a, 0x5a, 0x00),
            subject: "Sound mix, version 3",
            preview: "Version 3 of the mix is on the shared drive. I lowered the music under…",
            body: "Version 3 of the mix is on the shared drive. I lowered the music under the dialogue in scenes 2 and 5.",
            time: "Sat 16:20",
            group: Group::Yesterday,
            unread: false,
            flagged: false,
            chip: Some("No reply in 3 days"),
            attachments: Vec::new(),
        },
        Message {
            account: VIDEOTRON,
            from: "Neighbourhood Association",
            address: "news@neighbourhood.example",
            initials: "NA",
            avatar: rgb(0x7a, 0x4f, 0xc0),
            subject: "Winter parking schedule",
            preview: "Starting December 1, street parking alternates every night…",
            body: "Starting December 1, street parking alternates every night. The schedule for each street is attached to our website.",
            time: "Sat 11:05",
            group: Group::Yesterday,
            unread: true,
            flagged: false,
            chip: None,
            attachments: Vec::new(),
        },
        Message {
            account: OUTLOOK,
            from: "Studio Ops",
            address: "ops@studio.example",
            initials: "SO",
            avatar: rgb(0x2f, 0x6f, 0x8f),
            subject: "Render farm maintenance this weekend",
            preview: "The render farm will be offline from Saturday 08:00 to Sunday…",
            body: "The render farm will be offline from Saturday 08:00 to Sunday 20:00.",
            time: "Sat 09:30",
            group: Group::Yesterday,
            unread: false,
            flagged: false,
            chip: None,
            attachments: Vec::new(),
        },
        Message {
            account: GMAIL,
            from: "Jun Watanabe",
            address: "jun@studio.example",
            initials: "JW",
            avatar: rgb(0xb0, 0x45, 0x3a),
            subject: "Character turnarounds",
            preview: "Here are the first turnarounds for the fox. Front, side and back…",
            body: "Here are the first turnarounds for the fox. Front, side and back views; the three-quarter view follows tomorrow.",
            time: "Thu 17:40",
            group: Group::LastWeek,
            unread: true,
            flagged: false,
            chip: None,
            attachments: vec![Attachment { name: "fox-turnaround.png", kind: "PNG image", size: "1.8 MB" }],
        },
    ]
}

/// The folders shown under an expanded account, with their unread counts.
pub fn folders(provider: Provider) -> &'static [(&'static str, u32)] {
    match provider {
        Provider::Outlook => &[("Inbox", 1), ("Drafts", 1), ("Sent Items", 0), ("Archive", 0), ("Snoozed", 2), ("Junk Email", 2), ("Deleted Items", 0)],
        Provider::Gmail => &[("Inbox", 2), ("Drafts", 0), ("Sent", 0), ("All Mail", 0), ("Snoozed", 0), ("Spam", 4), ("Trash", 0)],
        Provider::Imap(_) => &[("Inbox", 1), ("Drafts", 0), ("Sent", 0), ("Archive", 0), ("Junk", 0), ("Trash", 0)],
    }
}
