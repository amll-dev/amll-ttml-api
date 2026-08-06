#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchType {
    NoMatch = 0,
    VeryLow = 1,
    Low = 2,
    Medium = 3,
    PrettyHigh = 4,
    High = 5,
    VeryHigh = 6,
    Perfect = 7,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameMatchType {
    NoMatch = 0,
    Low = 2,
    Medium = 4,
    High = 6,
    VeryHigh = 8,
    Perfect = 10,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtistMatchType {
    NoMatch = 0,
    Low = 2,
    Medium = 4,
    High = 6,
    VeryHigh = 8,
    Perfect = 10,
}
