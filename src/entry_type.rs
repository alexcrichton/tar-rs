// See https://en.wikipedia.org/wiki/Tar_%28computing%29#UStar_format
/// Indicate for the type of file described by a header.
///
/// Each `Header` has an `entry_type` method returning an instance of this type
/// which can be used to inspect what the header is describing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct EntryType {
    byte: u8,
}

impl EntryType {
    /// Creates a new entry type from a raw byte.
    ///
    /// Note that the other named constructors of entry type may be more
    /// appropriate to create a file type from.
    pub fn new(byte: u8) -> EntryType {
        EntryType { byte: byte }
    }

    /// Creates a new entry type representing a regular file.
    pub fn file() -> EntryType {
        EntryType::new(b'0')
    }

    /// Creates a new entry type representing a hard link.
    pub fn hard_link() -> EntryType {
        EntryType::new(b'1')
    }

    /// Creates a new entry type representing a symlink.
    pub fn symlink() -> EntryType {
        EntryType::new(b'2')
    }

    /// Creates a new entry type representing a character special device.
    pub fn character_special() -> EntryType {
        EntryType::new(b'3')
    }

    /// Creates a new entry type representing a block special device.
    pub fn block_special() -> EntryType {
        EntryType::new(b'4')
    }

    /// Creates a new entry type representing a directory.
    pub fn dir() -> EntryType {
        EntryType::new(b'5')
    }

    /// Creates a new entry type representing a FIFO.
    pub fn fifo() -> EntryType {
        EntryType::new(b'6')
    }

    /// Creates a new entry type representing a contiguous file.
    pub fn contiguous() -> EntryType {
        EntryType::new(b'7')
    }

    /// Returns whether this type represents a regular file.
    pub fn is_file(&self) -> bool {
        self.byte == 0 || self.byte == b'0'
    }

    /// Returns whether this type represents a hard link.
    pub fn is_hard_link(&self) -> bool {
        self.byte == b'1'
    }

    /// Returns whether this type represents a symlink.
    pub fn is_symlink(&self) -> bool {
        self.byte == b'2'
    }

    /// Returns whether this type represents a character special device.
    pub fn is_character_special(&self) -> bool {
        self.byte == b'3'
    }

    /// Returns whether this type represents a block special device.
    pub fn is_block_special(&self) -> bool {
        self.byte == b'4'
    }

    /// Returns whether this type represents a directory.
    pub fn is_dir(&self) -> bool {
        self.byte == b'5'
    }

    /// Returns whether this type represents a FIFO.
    pub fn is_fifo(&self) -> bool {
        self.byte == b'6'
    }

    /// Returns whether this type represents a contiguous file.
    pub fn is_contiguous(&self) -> bool {
        self.byte == b'7'
    }

    /// Returns the raw underlying byte that this entry type represents.
    pub fn as_byte(&self) -> u8 {
        self.byte
    }

    /// Returns whether this type represents a GNU long name header.
    pub fn is_gnu_longname(&self) -> bool {
        self.byte == b'L'
    }
}
