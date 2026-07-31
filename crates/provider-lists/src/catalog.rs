use crate::ListItemTag;

/// One list this runtime can actually mutate.
///
/// The catalog is pinned in Rust and is the single answer to
/// `lists.supported`. A kind absent from it is refused, never attempted — the
/// runtime does not guess at the shape of a list it has no contract for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SupportedList {
    pub kind: u16,
    /// NAP-LISTS type derived from the exact NIP-51 table name.
    pub list_type: &'static str,
    /// Item tags this list accepts. An item carrying any other tag is refused.
    pub item_types: &'static [ListItemTag],
    /// Parameterized replaceable lists (30000-39999) are addressed by a `d`
    /// identifier; the rest must not carry one.
    pub addressable: bool,
}

/// NIP-51 lists (plus the NIP-02 follow list) this runtime services.
///
/// Deliberately conservative: every entry here is a replaceable list whose
/// public tag set is the whole of its meaning, so a mutation is a pure
/// set operation. Lists whose semantics live in encrypted content are absent
/// rather than half-supported.
pub const SUPPORTED_LISTS: &[SupportedList] = &[
    SupportedList {
        kind: 3,
        list_type: "follow-list",
        item_types: &[ListItemTag::P],
        addressable: false,
    },
    SupportedList {
        kind: 10_000,
        list_type: "mute-list",
        item_types: &[ListItemTag::P, ListItemTag::E, ListItemTag::T],
        addressable: false,
    },
    SupportedList {
        kind: 10_001,
        list_type: "pinned-notes",
        item_types: &[ListItemTag::E],
        addressable: false,
    },
    SupportedList {
        kind: 10_003,
        list_type: "bookmarks",
        item_types: &[ListItemTag::E, ListItemTag::A],
        addressable: false,
    },
    SupportedList {
        kind: 10_015,
        list_type: "interests",
        item_types: &[ListItemTag::T, ListItemTag::A],
        addressable: false,
    },
    SupportedList {
        kind: 30_000,
        list_type: "follow-sets",
        item_types: &[ListItemTag::P],
        addressable: true,
    },
    SupportedList {
        kind: 30_003,
        list_type: "bookmark-sets",
        item_types: &[ListItemTag::E, ListItemTag::A],
        addressable: true,
    },
    SupportedList {
        kind: 30_015,
        list_type: "interest-sets",
        item_types: &[ListItemTag::T],
        addressable: true,
    },
];

pub fn supported_list(kind: u16) -> Option<&'static SupportedList> {
    SUPPORTED_LISTS.iter().find(|list| list.kind == kind)
}

pub fn supported_list_type(list_type: &str) -> Option<&'static SupportedList> {
    SUPPORTED_LISTS
        .iter()
        .find(|list| list.list_type == list_type)
}

pub fn semantic_item_type(tag: ListItemTag) -> &'static str {
    match tag {
        ListItemTag::P => "pubkey",
        ListItemTag::E => "event",
        ListItemTag::A => "address",
        ListItemTag::T => "hashtag",
    }
}

pub fn parse_semantic_item_type(item_type: &str) -> Option<ListItemTag> {
    match item_type {
        "pubkey" => Some(ListItemTag::P),
        "event" => Some(ListItemTag::E),
        "address" => Some(ListItemTag::A),
        "hashtag" => Some(ListItemTag::T),
        _ => None,
    }
}

impl SupportedList {
    pub fn accepts(&self, tag: ListItemTag) -> bool {
        self.item_types.contains(&tag)
    }
}
