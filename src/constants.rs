/// Films per calendar day at or above this count are considered a bulk-add event.
/// A typical film marathon tops out around 8–9 films; the real-world oracle cluster
/// was 86 films on a single day. 10 sits safely between casual use and bulk noise.
pub const BULK_DATE_THRESHOLD: usize = 10;
