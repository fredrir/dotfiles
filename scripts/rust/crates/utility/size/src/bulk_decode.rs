#![forbid(unsafe_code)]

use std::ffi::CStr;

// Darwin <sys/attr.h>; kept portable so the decoder can be fuzzed on Linux.
pub(crate) const NAME: u32 = 0x0000_0001;
pub(crate) const DEVID: u32 = 0x0000_0002;
pub(crate) const OBJTYPE: u32 = 0x0000_0008;
pub(crate) const ACCESSMASK: u32 = 0x0002_0000;
pub(crate) const FILEID: u32 = 0x0200_0000;
pub(crate) const RETURNED_ATTRS: u32 = 0x8000_0000;
pub(crate) const DIR_ALLOCSIZE: u32 = 0x0000_0008;
pub(crate) const LINKCOUNT: u32 = 0x0000_0001;
pub(crate) const FILE_ALLOCSIZE: u32 = 0x0000_0004;
pub(crate) const DATALENGTH: u32 = 0x0000_0200;
pub(crate) const COMMON: u32 = RETURNED_ATTRS | NAME | DEVID | OBJTYPE | ACCESSMASK | FILEID;
pub(crate) const FILE: u32 = LINKCOUNT | FILE_ALLOCSIZE | DATALENGTH;

#[derive(Debug)]
pub(crate) struct Entry<'a> {
    pub name: &'a CStr,
    pub devid: i32,
    pub objtype: u32,
    pub accessmask: u32,
    pub fileid: u64,
    pub linkcount: u32,
    pub allocated: u64,
    pub bytes: u64,
}

struct Fields<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl Fields<'_> {
    fn take<const N: usize>(&mut self) -> Option<[u8; N]> {
        let end = self.position.checked_add(N)?;
        let value = self.bytes.get(self.position..end)?.try_into().ok()?;
        self.position = end;
        Some(value)
    }

    fn u32(&mut self) -> Option<u32> {
        self.take().map(u32::from_ne_bytes)
    }

    fn optional_u32(&mut self, present: bool) -> Option<u32> {
        if present { self.u32() } else { Some(0) }
    }

    fn size(&mut self) -> Option<u64> {
        u64::try_from(i64::from_ne_bytes(self.take()?)).ok()
    }
}

pub(crate) fn decode(buffer: &[u8]) -> Option<(Entry<'_>, usize)> {
    let length = u32::from_ne_bytes(buffer.get(..4)?.try_into().ok()?) as usize;
    let record = buffer.get(..length)?;
    let mut fields = Fields {
        bytes: record,
        position: 4,
    };
    let common = fields.u32()?;
    let volume = fields.u32()?;
    let directory = fields.u32()?;
    let file = fields.u32()?;
    let fork = fields.u32()?;
    if common & (RETURNED_ATTRS | NAME) != RETURNED_ATTRS | NAME
        || common & !COMMON != 0
        || volume != 0
        || directory & !DIR_ALLOCSIZE != 0
        || file & !FILE != 0
        || fork != 0
    {
        return None;
    }

    let reference = fields.position;
    let offset = i32::from_ne_bytes(fields.take()?);
    let name_length = fields.u32()? as usize;
    let name_start = reference.checked_add_signed(offset as isize)?;
    let name_end = name_start.checked_add(name_length)?;

    let devid = fields.optional_u32(common & DEVID != 0)? as i32;
    let objtype = fields.optional_u32(common & OBJTYPE != 0)?;
    let accessmask = fields.optional_u32(common & ACCESSMASK != 0)?;
    let fileid = if common & FILEID != 0 {
        u64::from_ne_bytes(fields.take()?)
    } else {
        0
    };
    let mut allocated = if directory & DIR_ALLOCSIZE != 0 {
        Some(fields.size()?)
    } else {
        None
    };
    let linkcount = fields.optional_u32(file & LINKCOUNT != 0)?;
    if file & FILE_ALLOCSIZE != 0 {
        allocated = Some(fields.size()?);
    }
    let bytes = if file & DATALENGTH != 0 {
        fields.size()?
    } else {
        0
    };

    // The variable attribute must not overlap the fixed attributes or escape
    // its own record, even when the surrounding batch has more bytes.
    if name_start < fields.position {
        return None;
    }
    let name = CStr::from_bytes_with_nul(record.get(name_start..name_end)?).ok()?;
    if name.is_empty() {
        return None;
    }
    Some((
        Entry {
            name,
            devid,
            objtype,
            accessmask,
            fileid,
            linkcount,
            allocated: allocated.unwrap_or(bytes),
            bytes,
        },
        length,
    ))
}
