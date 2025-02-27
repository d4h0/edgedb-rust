use std::convert::{TryFrom, TryInto};
use std::sync::Arc;

use bytes::Buf;
use uuid::Uuid;
use snafu::{ensure, OptionExt};

use crate::codec::{Codec, build_codec};
use crate::common::Cardinality;
use crate::encoding::{Decode, Input};
use crate::errors::{InvalidTypeDescriptor, UnexpectedTypePos};
use crate::errors::{self, DecodeError, CodecError};
use crate::features::ProtocolVersion;
use crate::queryable;
use crate::query_arg;


#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct TypePos(pub u16);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Descriptor {
    Set(SetDescriptor),
    ObjectShape(ObjectShapeDescriptor),
    BaseScalar(BaseScalarTypeDescriptor),
    Scalar(ScalarTypeDescriptor),
    Tuple(TupleTypeDescriptor),
    NamedTuple(NamedTupleTypeDescriptor),
    Array(ArrayTypeDescriptor),
    Range(RangeTypeDescriptor),
    Enumeration(EnumerationTypeDescriptor),
    InputShape(InputShapeTypeDescriptor),
    TypeAnnotation(TypeAnnotationDescriptor),
}

pub struct OutputTypedesc {
    pub(crate) proto: ProtocolVersion,
    pub(crate) array: Vec<Descriptor>,
    #[allow(dead_code)] // TODO
    pub(crate) root_id: Uuid,
    pub(crate) root_pos: Option<TypePos>,
}

pub struct InputTypedesc {
    pub(crate) proto: ProtocolVersion,
    pub(crate) array: Vec<Descriptor>,
    #[allow(dead_code)] // TODO
    pub(crate) root_id: Uuid,
    pub(crate) root_pos: Option<TypePos>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetDescriptor {
    pub id: Uuid,
    pub type_pos: TypePos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectShapeDescriptor {
    pub id: Uuid,
    pub elements: Vec<ShapeElement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InputShapeTypeDescriptor {
    pub id: Uuid,
    pub elements: Vec<ShapeElement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShapeElement {
    pub flag_implicit: bool,
    pub flag_link_property: bool,
    pub flag_link: bool,
    pub cardinality: Option<Cardinality>,
    pub name: String,
    pub type_pos: TypePos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseScalarTypeDescriptor {
    pub id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScalarTypeDescriptor {
    pub id: Uuid,
    pub base_type_pos: TypePos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TupleTypeDescriptor {
    pub id: Uuid,
    pub element_types: Vec<TypePos>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedTupleTypeDescriptor {
    pub id: Uuid,
    pub elements: Vec<TupleElement>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TupleElement {
    pub name: String,
    pub type_pos: TypePos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArrayTypeDescriptor {
    pub id: Uuid,
    pub type_pos: TypePos,
    pub dimensions: Vec<Option<u32>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeTypeDescriptor {
    pub id: Uuid,
    pub type_pos: TypePos,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumerationTypeDescriptor {
    pub id: Uuid,
    pub members: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeAnnotationDescriptor {
    pub annotated_type: u8,
    pub id: Uuid,
    pub annotation: String,
}

impl OutputTypedesc {
    pub fn as_queryable_context(&self) -> queryable::DescriptorContext {
        let mut ctx = queryable::DescriptorContext::new(self.descriptors());
        ctx.has_implicit_tid = self.proto.has_implicit_tid();
        ctx
    }
    pub fn descriptors(&self) -> &[Descriptor] {
        &self.array
    }
    pub fn build_codec(&self) -> Result<Arc<dyn Codec>, CodecError> {
        build_codec(self.root_pos(), self.descriptors())
    }
    pub fn root_pos(&self) -> Option<TypePos> {
        self.root_pos
    }
    pub fn decode_with_id(root_id: Uuid, buf: &mut Input) -> Result<Self, DecodeError> {
        let mut descriptors = Vec::new();
        while buf.remaining() > 0 {
            match Descriptor::decode(buf)? {
                Descriptor::TypeAnnotation(_) => {}
                item => descriptors.push(item),
            }
        }
        let root_pos = if root_id == Uuid::from_u128(0) {
            None
        } else {
            let idx = descriptors.iter().position(|x| *x.id() == root_id)
                .context(errors::UuidNotFound { uuid: root_id })?;
            let pos = idx.try_into().ok()
                .context(errors::TooManyDescriptors { index: idx })?;
            Some(TypePos(pos))
        };
        Ok(OutputTypedesc {
            proto: buf.proto().clone(),
            array: descriptors,
            root_id,
            root_pos,
        })
    }
}


impl InputTypedesc {
    pub fn as_query_arg_context(&self) -> query_arg::DescriptorContext {
        query_arg::DescriptorContext {
            proto: &self.proto,
            descriptors: self.descriptors(),
            root_pos: self.root_pos,
        }
    }
    pub fn descriptors(&self) -> &[Descriptor] {
        &self.array
    }
    pub fn build_codec(&self) -> Result<Arc<dyn Codec>, CodecError> {
        build_codec(self.root_pos(), self.descriptors())
    }
    pub fn root(&self) -> Option<&Descriptor> {
        self.root_pos.and_then(|pos| self.array.get(pos.0 as usize))
    }
    pub fn root_pos(&self) -> Option<TypePos> {
        self.root_pos
    }
    pub fn get(&self, type_pos: TypePos) -> Result<&Descriptor, CodecError> {
        self.array.get(type_pos.0 as usize)
            .context(UnexpectedTypePos { position: type_pos.0 })
    }
    pub fn is_empty_tuple(&self) -> bool {
        match self.root() {
            Some(Descriptor::Tuple(t))
              => t.id == Uuid::from_u128(0xFF) && t.element_types.is_empty(),
            _ => false,
        }
    }
    pub fn proto(&self) -> &ProtocolVersion {
        &self.proto
    }
}

impl Descriptor {
    pub fn id(&self) -> &Uuid {
        use Descriptor::*;
        match self {
            Set(i) => &i.id,
            ObjectShape(i) => &i.id,
            BaseScalar(i) => &i.id,
            Scalar(i) => &i.id,
            Tuple(i) => &i.id,
            NamedTuple(i) => &i.id,
            Array(i) => &i.id,
            Range(i) => &i.id,
            Enumeration(i) => &i.id,
            InputShape(i) => &i.id,
            TypeAnnotation(i) => &i.id,
        }
    }
    pub fn decode(buf: &mut Input) -> Result<Descriptor, DecodeError> {
        <Descriptor as Decode>::decode(buf)
    }
}

impl Decode for Descriptor {
    fn decode(buf: &mut Input) -> Result<Self, DecodeError> {
        use Descriptor as D;
        ensure!(buf.remaining() >= 1, errors::Underflow);
        match buf.chunk()[0] {
            0 => SetDescriptor::decode(buf).map(D::Set),
            1 => ObjectShapeDescriptor::decode(buf).map(D::ObjectShape),
            2 => BaseScalarTypeDescriptor::decode(buf).map(D::BaseScalar),
            3 => ScalarTypeDescriptor::decode(buf).map(D::Scalar),
            4 => TupleTypeDescriptor::decode(buf).map(D::Tuple),
            5 => NamedTupleTypeDescriptor::decode(buf).map(D::NamedTuple),
            6 => ArrayTypeDescriptor::decode(buf).map(D::Array),
            7 => EnumerationTypeDescriptor::decode(buf).map(D::Enumeration),
            8 => InputShapeTypeDescriptor::decode(buf).map(D::InputShape),
            9 => RangeTypeDescriptor::decode(buf).map(D::Range),
            0x7F..=0xFF => {
                TypeAnnotationDescriptor::decode(buf).map(D::TypeAnnotation)
            }
            descriptor => InvalidTypeDescriptor { descriptor }.fail()?
        }
    }
}

impl Decode for SetDescriptor {
    fn decode(buf: &mut Input) -> Result<Self, DecodeError> {
        ensure!(buf.remaining() >= 19, errors::Underflow);
        assert!(buf.get_u8() == 0);
        let id = Uuid::decode(buf)?;
        let type_pos = TypePos(buf.get_u16());
        Ok(SetDescriptor { id, type_pos })
    }
}

impl Decode for ObjectShapeDescriptor {
    fn decode(buf: &mut Input) -> Result<Self, DecodeError> {
        ensure!(buf.remaining() >= 19, errors::Underflow);
        assert!(buf.get_u8() == 1);
        let id = Uuid::decode(buf)?;
        let element_count = buf.get_u16();
        let mut elements = Vec::with_capacity(element_count as usize);
        for _ in 0..element_count {
            elements.push(ShapeElement::decode(buf)?);
        }
        Ok(ObjectShapeDescriptor { id, elements })
    }
}

impl Decode for InputShapeTypeDescriptor {
    fn decode(buf: &mut Input) -> Result<Self, DecodeError> {
        ensure!(buf.remaining() >= 19, errors::Underflow);
        assert!(buf.get_u8() == 8);
        let id = Uuid::decode(buf)?;
        let element_count = buf.get_u16();
        let mut elements = Vec::with_capacity(element_count as usize);
        for _ in 0..element_count {
            elements.push(ShapeElement::decode(buf)?);
        }
        Ok(InputShapeTypeDescriptor { id, elements })
    }
}

impl Decode for ShapeElement {
    fn decode(buf: &mut Input) -> Result<Self, DecodeError> {
        ensure!(buf.remaining() >= 7, errors::Underflow);
        let (flags, cardinality) = if buf.proto().is_at_least(0, 11) {
            let flags = buf.get_u32();
            let cardinality = TryFrom::try_from(buf.get_u8())?;
            (flags, Some(cardinality))
        } else {
            (buf.get_u8() as u32, None)
        };
        let name = String::decode(buf)?;
        ensure!(buf.remaining() >= 2, errors::Underflow);
        let type_pos = TypePos(buf.get_u16());
        Ok(ShapeElement {
            flag_implicit: flags & 0b001 != 0,
            flag_link_property: flags & 0b010 != 0,
            flag_link: flags & 0b100 != 0,
            cardinality,
            name,
            type_pos,
        })
    }
}

impl Decode for BaseScalarTypeDescriptor {
    fn decode(buf: &mut Input) -> Result<Self, DecodeError> {
        assert!(buf.get_u8() == 2);
        let id = Uuid::decode(buf)?;
        Ok(BaseScalarTypeDescriptor { id })
    }
}


impl Decode for ScalarTypeDescriptor {
    fn decode(buf: &mut Input) -> Result<Self, DecodeError> {
        ensure!(buf.remaining() >= 19, errors::Underflow);
        assert!(buf.get_u8() == 3);
        let id = Uuid::decode(buf)?;
        let base_type_pos = TypePos(buf.get_u16());
        Ok(ScalarTypeDescriptor { id, base_type_pos })
    }
}

impl Decode for TupleTypeDescriptor {
    fn decode(buf: &mut Input) -> Result<Self, DecodeError> {
        ensure!(buf.remaining() >= 19, errors::Underflow);
        assert!(buf.get_u8() == 4);
        let id = Uuid::decode(buf)?;
        let el_count = buf.get_u16();
        ensure!(buf.remaining() >= 2*el_count as usize, errors::Underflow);
        let mut element_types = Vec::with_capacity(el_count as usize);
        for _ in 0..el_count {
            element_types.push(TypePos(buf.get_u16()));
        }
        Ok(TupleTypeDescriptor { id, element_types })
    }
}

impl Decode for NamedTupleTypeDescriptor {
    fn decode(buf: &mut Input) -> Result<Self, DecodeError> {
        ensure!(buf.remaining() >= 19, errors::Underflow);
        assert!(buf.get_u8() == 5);
        let id = Uuid::decode(buf)?;
        let element_count = buf.get_u16();
        let mut elements = Vec::with_capacity(element_count as usize);
        for _ in 0..element_count {
            elements.push(TupleElement::decode(buf)?);
        }
        Ok(NamedTupleTypeDescriptor { id, elements })
    }
}

impl Decode for TupleElement {
    fn decode(buf: &mut Input) -> Result<Self, DecodeError> {
        let name = String::decode(buf)?;
        ensure!(buf.remaining() >= 2, errors::Underflow);
        let type_pos = TypePos(buf.get_u16());
        Ok(TupleElement {
            name,
            type_pos,
        })
    }
}

impl Decode for ArrayTypeDescriptor {
    fn decode(buf: &mut Input) -> Result<Self, DecodeError> {
        ensure!(buf.remaining() >= 21, errors::Underflow);
        assert!(buf.get_u8() == 6);
        let id = Uuid::decode(buf)?;
        let type_pos = TypePos(buf.get_u16());
        let dim_count = buf.get_u16();
        ensure!(buf.remaining() >= 4*dim_count as usize, errors::Underflow);
        let mut dimensions = Vec::with_capacity(dim_count as usize);
        for _ in 0..dim_count {
            dimensions.push(match buf.get_i32() {
                -1 => None,
                n if n > 0 => Some(n as u32),
                _ => errors::InvalidArrayShape.fail()?,
            });
        }
        Ok(ArrayTypeDescriptor { id, type_pos, dimensions })
    }
}

impl Decode for RangeTypeDescriptor {
    fn decode(buf: &mut Input) -> Result<Self, DecodeError> {
        ensure!(buf.remaining() >= 19, errors::Underflow);
        assert!(buf.get_u8() == 9);
        let id = Uuid::decode(buf)?;
        let type_pos = TypePos(buf.get_u16());
        Ok(RangeTypeDescriptor { id, type_pos })
    }
}

impl Decode for EnumerationTypeDescriptor {
    fn decode(buf: &mut Input) -> Result<Self, DecodeError> {
        ensure!(buf.remaining() >= 19, errors::Underflow);
        assert!(buf.get_u8() == 7);
        let id = Uuid::decode(buf)?;
        let member_count = buf.get_u16();
        let mut members = Vec::with_capacity(member_count as usize);
        for _ in 0..member_count {
            members.push(String::decode(buf)?);
        }
        Ok(EnumerationTypeDescriptor { id, members })
    }
}

impl Decode for TypeAnnotationDescriptor {
    fn decode(buf: &mut Input) -> Result<Self, DecodeError> {
        ensure!(buf.remaining() >= 21, errors::Underflow);
        let annotated_type = buf.get_u8();
        assert!(annotated_type >= 0x7F);
        let id = Uuid::decode(buf)?;
        let annotation = String::decode(buf)?;
        Ok(TypeAnnotationDescriptor { annotated_type, id, annotation })
    }
}
