//! A simple example of parsing `.debug_info`.

use object::{Object, ObjectSection};
use std::{borrow, env, fs};
use gimli;
use gimli::{CompilationUnitHeader, Section, UnitOffset, UnitSectionOffset, UnwindSection};
//use std::io::{BufWriter, Write};
use std::io;
use std::fmt::Write;
use std::collections::HashMap;


fn main() {
    for path in env::args().skip(1) {
        let file = fs::File::open(&path).unwrap();
        let mmap = unsafe { memmap::Mmap::map(&file).unwrap() };
        let object = object::File::parse(&*mmap).unwrap();
        let endian = if object.is_little_endian() {
            gimli::RunTimeEndian::Little
        } else {
            gimli::RunTimeEndian::Big
        };
        dump_file(&object, endian).unwrap();
    }
}

#[derive(Debug, Clone)]
pub struct Type {
  name: String,
  byte_size: u64,
}

impl Type {
  pub fn new(name: String, byte_size: u64) -> Self {
    Type {name: name, byte_size: byte_size}
  }
}

#[derive(Debug)]
pub struct Variable {
  name: String,
  entity_type: Type,
  location: usize,
  filename: String,
  line_number: usize,
}


fn dump_file(object: &object::File, endian: gimli::RunTimeEndian) -> Result<(), Error> {
    // Load a section and return as `Cow<[u8]>`.
    let load_section = |id: gimli::SectionId| -> Result<borrow::Cow<[u8]>, gimli::Error> {
        match object.section_by_name(id.name()) {
            Some(ref section) => Ok(section
                .uncompressed_data()
                .unwrap_or(borrow::Cow::Borrowed(&[][..]))),
            None => Ok(borrow::Cow::Borrowed(&[][..])),
        }
    };
    // Load a supplementary section. We don't have a supplementary object file,
    // so always return an empty slice.
    let load_section_sup = |_| Ok(borrow::Cow::Borrowed(&[][..]));

    // Load all of the sections.
    let dwarf_cow = gimli::Dwarf::load(&load_section, &load_section_sup)?;

    // Borrow a `Cow<[u8]>` to create an `EndianSlice`.
    let borrow_section: &dyn for<'a> Fn(
        &'a borrow::Cow<[u8]>,
    ) -> gimli::EndianSlice<'a, gimli::RunTimeEndian> =
        &|section| gimli::EndianSlice::new(&*section, endian);

    // Create `EndianSlice`s for all of the sections.
    let dwarf = dwarf_cow.borrow(&borrow_section);

    // Define a mapping from type offsets to type structs
    let mut offset_to_type: HashMap<usize, Type> = HashMap::new();

    // Iterate over the compilation units.
    let mut iter = dwarf.units();
    while let Some(header) = iter.next()? {
        println!("Unit at <.debug_info+0x{:x}>", header.offset().0);
        let unit = dwarf.unit(header)?;

        // Iterate over the Debugging Information Entries (DIEs) in the unit.
        let mut depth = 0;
        let mut entries = unit.entries();
        while let Some((delta_depth, entry)) = entries.next_dfs()? {
            depth += delta_depth;
            println!("<{}><{:x}> {}", depth, entry.offset().0, entry.tag());
            // Update the offset_to_type mapping 
            match entry.tag() {
                gimli::DW_TAG_base_type => {
                    let name = if let Ok(Some(attr)) = entry.attr(gimli::DW_AT_name) {
                        if let Ok(DebugValue::Str(name)) =
                            get_attr_value(&attr, &unit, &dwarf) {
                            name
                        } else {
                            "<unknown>".to_string()
                        }
                    } else {
                        "<unknown>".to_string()
                    };
                    let byte_size  = if let Ok(Some(attr)) =
                            entry.attr(gimli::DW_AT_byte_size) {
                        if let Ok(DebugValue::Uint(byte_size)) =
                            get_attr_value(&attr, &unit, &dwarf) {
                            byte_size
                        } else {
                            // TODO: report error?
                            0
                        }
                    } else {
                        // TODO: report error?
                        0
                    };
                    let type_offset = entry.offset().0;
                    offset_to_type.insert(type_offset, Type::new(name, byte_size));
                }, // TODO: add other types?
                _ => {},
            } 
            // Iterate over the attributes in the DIE.
            let mut attrs = entry.attrs();
            while let Some(attr) = attrs.next()? {
                let val = get_attr_value(&attr, &unit, &dwarf);
                println!("   {}: {:?}", attr.name(), val);
                if let gimli::DW_AT_type = attr.name() {
                    if let Ok(DebugValue::Size(offset)) = val {
                        println!("type: {:?}", offset_to_type.get(&offset));
                    }
                }
            }
        }
    }
    println!("offset_to_type: {:?}", offset_to_type);
    Ok(())
}

#[derive(Debug, Clone)]
pub enum DebugValue {
  Str(String), Uint(u64), Size(usize), NoVal,
  
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    GimliError(gimli::Error),
    ObjectError(object::read::Error),
    IoError,
}

impl From<gimli::Error> for Error {
    fn from(err: gimli::Error) -> Self {
        Error::GimliError(err)
    }
}

impl From<io::Error> for Error {
    fn from(_: io::Error) -> Self {
        Error::IoError
    }
}


impl From<std::fmt::Error> for Error {
    fn from(_: std::fmt::Error) -> Self {
        Error::IoError
    }
}

impl From<object::read::Error> for Error {
    fn from(err: object::read::Error) -> Self {
        Error::ObjectError(err)
    }
}

impl<'input, Endian> Reader for gimli::EndianSlice<'input, Endian> where
    Endian: gimli::Endianity + Send + Sync
{
}

trait Reader: gimli::Reader<Offset = usize> + Send + Sync {}

// based on dwarf_dump.rs
fn get_attr_value<R: Reader>(
    attr: &gimli::Attribute<R>,
    unit: &gimli::Unit<R>,
    dwarf: &gimli::Dwarf<R>,
) -> Result<DebugValue, Error> {
    let value = attr.value();
    // TODO: get rid of w eventually
    let mut buf = String::new();
    let w = &mut buf;
    match value {
        gimli::AttributeValue::Exprloc(ref data) => {
            if let gimli::AttributeValue::Exprloc(_) = attr.raw_value() {
                write!(w, "len 0x{:04x}: ", data.0.len());
                for byte in data.0.to_slice()?.iter() {
                    write!(w, "{:02x}", byte);
                }
                write!(w, ": ");
            }
            dump_exprloc(w, unit.encoding(), data);
            Ok(DebugValue::Str(w.to_string()))
        }
        // TODO: keep below
        gimli::AttributeValue::UnitRef(offset) => {
            write!(w, "0x{:08x}", offset.0);
            match offset.to_unit_section_offset(unit) {
                UnitSectionOffset::DebugInfoOffset(goff) => {
                    //write!(w, "<.debug_info+0x{:08x}>", goff.0);
                    Ok(DebugValue::Size(goff.0))
                }
                UnitSectionOffset::DebugTypesOffset(goff) => {
                    //write!(w, "<.debug_types+0x{:08x}>", goff.0);
                    Ok(DebugValue::Size(goff.0))
                }
            }
            //Ok(DebugValue::Str(w.to_string()))
        }
        // TODO: KEEP DebugStrRef
        gimli::AttributeValue::DebugStrRef(offset) => {
            if let Ok(s) = dwarf.debug_str.get_str(offset) {
                Ok(DebugValue::Str(format!("{}", s.to_string_lossy()?)))
            } else {
                Ok(DebugValue::Str(format!("<.debug_str+0x{:08x}>", offset.0)))
            }
        }
  
        /*gimli::AttributeValue::Encoding(value) => {
            write!(w, "{}", value)?;
            Ok(DebugValue::Str(w.to_string()))
        }*/

        gimli::AttributeValue::Udata(data) => {
            /*match attr.name() {
                gimli::DW_AT_high_pc => {
                    write!(w, "<offset-from-lowpc>{}", data);
                }
                gimli::DW_AT_data_member_location => {
                    if let Some(sdata) = attr.sdata_value() {
                        // This is a DW_FORM_data* value.
                        // libdwarf-dwarfdump displays this as signed too.
                        if sdata >= 0 {
                            write!(w, "{}", data);
                        } else {
                            write!(w, "{} ({})", data, sdata);
                        }
                    } else {
                        write!(w, "{}", data);
                    }
                }
                gimli::DW_AT_lower_bound | gimli::DW_AT_upper_bound => {
                    write!(w, "{}", data);
                }
                _ => {
                    write!(w, "0x{:08x}", data);
                }
            };
            Ok(DebugValue::Str(w.to_string()))*/
            Ok(DebugValue::Uint(data))
        }

        gimli::AttributeValue::String(s) => {
            // TODO: remove unwrap!
            write!(w, "{}", s.to_string_lossy().unwrap());
            Ok(DebugValue::Str(w.to_string()))
        }
        gimli::AttributeValue::FileIndex(value) => {
            write!(w, "0x{:08x}", value);
            dump_file_index(w, value, unit, dwarf);
            Ok(DebugValue::Str(w.to_string()))
        }
        _ => { // Don't handle other values
            Ok(DebugValue::NoVal)
        }
    }
}


fn dump_file_index<R: Reader, W: Write>(
    w: &mut W,
    file: u64,
    unit: &gimli::Unit<R>,
    dwarf: &gimli::Dwarf<R>,
) -> Result<(), Error> {
    if file == 0 {
        return Ok(());
    }
    let header = match unit.line_program {
        Some(ref program) => program.header(),
        None => return Ok(()),
    };
    let file = match header.file(file) {
        Some(header) => header,
        None => {
            writeln!(w, "Unable to get header for file {}", file)?;
            return Ok(());
        }
    };
    write!(w, " ")?;
    if let Some(directory) = file.directory(header) {
        let directory = dwarf.attr_string(unit, directory)?;
        let directory = directory.to_string_lossy()?;
        if !directory.starts_with('/') {
            if let Some(ref comp_dir) = unit.comp_dir {
                write!(w, "{}/", comp_dir.to_string_lossy()?,)?;
            }
        }
        write!(w, "{}/", directory)?;
    }
    write!(
        w,
        "{}",
        dwarf
            .attr_string(unit, file.path_name())?
            .to_string_lossy()?
    )?;
    Ok(())
}

fn dump_exprloc<R: Reader, W: Write>(
    w: &mut W,
    encoding: gimli::Encoding,
    data: &gimli::Expression<R>,
) -> Result<(), Error> {
    let mut pc = data.0.clone();
    let mut space = false;
    while pc.len() != 0 {
        let mut op_pc = pc.clone();
        let dwop = gimli::DwOp(op_pc.read_u8()?);
        match gimli::Operation::parse(&mut pc, encoding) {
            Ok(op) => {
                if space {
                    write!(w, " ")?;
                } else {
                    space = true;
                }
                dump_op(w, encoding, dwop, op)?;
            }
            Err(gimli::Error::InvalidExpression(op)) => {
                writeln!(w, "WARNING: unsupported operation 0x{:02x}", op.0)?;
                return Ok(());
            }
            Err(gimli::Error::UnsupportedRegister(register)) => {
                writeln!(w, "WARNING: unsupported register {}", register)?;
                return Ok(());
            }
            Err(gimli::Error::UnexpectedEof(_)) => {
                writeln!(w, "WARNING: truncated or malformed expression")?;
                return Ok(());
            }
            Err(e) => {
                writeln!(w, "WARNING: unexpected operation parse error: {}", e)?;
                return Ok(());
            }
        }
    }
    Ok(())
}


fn dump_op<R: Reader, W: Write>(
    w: &mut W,
    encoding: gimli::Encoding,
    dwop: gimli::DwOp,
    op: gimli::Operation<R>,
) -> Result<(), Error> {
    write!(w, "{}", dwop)?;
    match op {
        gimli::Operation::Deref {
            base_type, size, ..
        } => {
            if dwop == gimli::DW_OP_deref_size || dwop == gimli::DW_OP_xderef_size {
                write!(w, " {}", size)?;
            }
            if base_type != UnitOffset(0) {
                write!(w, " type 0x{:08x}", base_type.0)?;
            }
        }
        gimli::Operation::Pick { index } => {
            if dwop == gimli::DW_OP_pick {
                write!(w, " {}", index)?;
            }
        }
        gimli::Operation::PlusConstant { value } => {
            write!(w, " {}", value as i64)?;
        }
        gimli::Operation::Bra { target } => {
            write!(w, " {}", target)?;
        }
        gimli::Operation::Skip { target } => {
            write!(w, " {}", target)?;
        }
        gimli::Operation::SignedConstant { value } => match dwop {
            gimli::DW_OP_const1s
            | gimli::DW_OP_const2s
            | gimli::DW_OP_const4s
            | gimli::DW_OP_const8s
            | gimli::DW_OP_consts => {
                write!(w, " {}", value)?;
            }
            _ => {}
        },
        gimli::Operation::UnsignedConstant { value } => match dwop {
            gimli::DW_OP_const1u
            | gimli::DW_OP_const2u
            | gimli::DW_OP_const4u
            | gimli::DW_OP_const8u
            | gimli::DW_OP_constu => {
                write!(w, " {}", value)?;
            }
            _ => {
                // These have the value encoded in the operation, eg DW_OP_lit0.
            }
        },
        gimli::Operation::Register { register } => {
            if dwop == gimli::DW_OP_regx {
                write!(w, " {}", register.0)?;
            }
        }
        gimli::Operation::RegisterOffset {
            register,
            offset,
            base_type,
        } => {
            if dwop >= gimli::DW_OP_breg0 && dwop <= gimli::DW_OP_breg31 {
                write!(w, "{:+}", offset)?;
            } else {
                write!(w, " {}", register.0)?;
                if offset != 0 {
                    write!(w, "{:+}", offset)?;
                }
                if base_type != UnitOffset(0) {
                    write!(w, " type 0x{:08x}", base_type.0)?;
                }
            }
        }
        gimli::Operation::FrameOffset { offset } => {
            write!(w, " {}", offset)?;
        }
        gimli::Operation::Call { offset } => match offset {
            gimli::DieReference::UnitRef(gimli::UnitOffset(offset)) => {
                write!(w, " 0x{:08x}", offset)?;
            }
            gimli::DieReference::DebugInfoRef(gimli::DebugInfoOffset(offset)) => {
                write!(w, " 0x{:08x}", offset)?;
            }
        },
        gimli::Operation::Piece {
            size_in_bits,
            bit_offset: None,
        } => {
            write!(w, " {}", size_in_bits / 8)?;
        }
        gimli::Operation::Piece {
            size_in_bits,
            bit_offset: Some(bit_offset),
        } => {
            write!(w, " 0x{:08x} offset 0x{:08x}", size_in_bits, bit_offset)?;
        }
        gimli::Operation::ImplicitValue { data } => {
            let data = data.to_slice()?;
            write!(w, " 0x{:08x} contents 0x", data.len())?;
            for byte in data.iter() {
                write!(w, "{:02x}", byte)?;
            }
        }
        gimli::Operation::ImplicitPointer { value, byte_offset } => {
            write!(w, " 0x{:08x} {}", value.0, byte_offset)?;
        }
        gimli::Operation::EntryValue { expression } => {
            write!(w, "(")?;
            dump_exprloc(w, encoding, &gimli::Expression(expression))?;
            write!(w, ")")?;
        }
        gimli::Operation::ParameterRef { offset } => {
            write!(w, " 0x{:08x}", offset.0)?;
        }
        gimli::Operation::Address { address } => {
            write!(w, " 0x{:08x}", address)?;
        }
        gimli::Operation::AddressIndex { index } => {
            write!(w, " 0x{:08x}", index.0)?;
        }
        gimli::Operation::ConstantIndex { index } => {
            write!(w, " 0x{:08x}", index.0)?;
        }
        gimli::Operation::TypedLiteral { base_type, value } => {
            write!(w, " type 0x{:08x} contents 0x", base_type.0)?;
            for byte in value.to_slice()?.iter() {
                write!(w, "{:02x}", byte)?;
            }
        }
        gimli::Operation::Convert { base_type } => {
            write!(w, " type 0x{:08x}", base_type.0)?;
        }
        gimli::Operation::Reinterpret { base_type } => {
            write!(w, " type 0x{:08x}", base_type.0)?;
        }
        gimli::Operation::Drop
        | gimli::Operation::Swap
        | gimli::Operation::Rot
        | gimli::Operation::Abs
        | gimli::Operation::And
        | gimli::Operation::Div
        | gimli::Operation::Minus
        | gimli::Operation::Mod
        | gimli::Operation::Mul
        | gimli::Operation::Neg
        | gimli::Operation::Not
        | gimli::Operation::Or
        | gimli::Operation::Plus
        | gimli::Operation::Shl
        | gimli::Operation::Shr
        | gimli::Operation::Shra
        | gimli::Operation::Xor
        | gimli::Operation::Eq
        | gimli::Operation::Ge
        | gimli::Operation::Gt
        | gimli::Operation::Le
        | gimli::Operation::Lt
        | gimli::Operation::Ne
        | gimli::Operation::Nop
        | gimli::Operation::PushObjectAddress
        | gimli::Operation::TLS
        | gimli::Operation::CallFrameCFA
        | gimli::Operation::StackValue => {}
    };
    Ok(())
}
