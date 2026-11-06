//! M04 metadata and links: the parse/resolve core behind tags, properties,
//! headings, internal links, backlinks and embeds.
//!
//! Everything in this crate is pure: it reads note sources and a note catalog
//! and returns derived data. Persistence belongs to M02 and the derived index
//! is rebuilt from sources rather than stored (F08-R02), so a stale index can
//! never outlive an edit.
//!
//! Scope: F08 tag parsing, F09 typed properties, F10 link/backlink resolution
//! and rename planning, F11 anchors and embed limits, plus the shared scanner.
//! `title_of` lives here because both the index and the host adapter need it.

pub mod headings;
pub mod index;
pub mod links;
pub mod properties;
mod property_write;
pub mod rename;
pub mod resolve;
pub mod scan;
pub mod tag_plan;
mod tags;

pub use headings::{
    anchors_of, extract_block_anchors, extract_headings, heading_slug, BlockAnchor, Heading,
};
pub use index::{
    content_for_anchor, Backlink, EmbedExpansion, EmbedStatus, LinkIssue, NoteMetadata, NoteSource,
    ParsedLink, VaultIndex, EMBED_DEPTH_LIMIT,
};
pub use links::{extract_links, LinkKind, RawLink};
pub use properties::{
    parse_properties, Property, PropertyError, PropertySet, PropertyType, PropertyValue,
};
pub use property_write::{
    render_value, set_property, PropertyWrite, PropertyWriteError, MAX_PROPERTY_NAME_CHARS,
    RESERVED_ID_KEY,
};
pub use rename::{
    apply_plan, link_target_for, plan_commit, plan_rename, title_of, CommitIssue, CommitStep,
    RenameCommit, RenameEdit, RenamePlan, RenameWarning, RenameWarningReason,
};
pub use resolve::{
    anchor_in, anchor_matches, resolve_link, resolve_link_with_anchors, Catalog, LinkStatus,
    NoteRef, ResolvedLink,
};
pub use scan::{Frontmatter, Scanner, Span};
pub use tag_plan::{
    plan_removal, plan_rename as plan_tag_rename, TagNote, TagPlanIssue, TagRenamePlan, TagRewrite,
};
pub use tags::{extract_tags, validate_tag, TagIssue, TagOccurrence, TagSource, MAX_TAG_CHARS};
