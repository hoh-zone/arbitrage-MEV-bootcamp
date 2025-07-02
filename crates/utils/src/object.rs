use eyre::{bail, OptionExt, Result};
use move_core_types::annotated_value::{MoveStruct, MoveValue};
use sui_types::{
    base_types::{ObjectID, SequenceNumber},
    dynamic_field::extract_field_from_move_struct,
    object::{Object, Owner},
    transaction::ObjectArg,
};

pub fn extract_struct_from_move_struct(move_struct: &MoveStruct, field_name: &str) -> Result<MoveStruct> {
    let move_value = extract_field_from_move_struct(move_struct, field_name).ok_or_eyre("field not found")?;

    match move_value {
        MoveValue::Struct(move_struct) => Ok(move_struct.clone()),
        _ => bail!("expected struct"),
    }
}

pub fn extract_vec_from_move_struct(move_struct: &MoveStruct, field_name: &str) -> Result<Vec<MoveValue>> {
    let move_value = extract_field_from_move_struct(move_struct, field_name).ok_or_eyre("field not found")?;

    match move_value {
        MoveValue::Vector(move_vec) => Ok(move_vec.clone()),
        _ => bail!("expected vector"),
    }
}

pub fn extract_object_id_from_move_struct(move_struct: &MoveStruct, field_name: &str) -> Result<ObjectID> {
    let move_value = extract_field_from_move_struct(move_struct, field_name).ok_or_eyre("field not found")?;

    match move_value {
        MoveValue::Address(addr) => Ok(ObjectID::from_address(*addr)),
        _ => bail!("expected address"),
    }
}

pub fn extract_struct_array_from_move_struct(move_struct: &MoveStruct, field_name: &str) -> Result<Vec<MoveStruct>> {
    let move_value = extract_field_from_move_struct(move_struct, field_name).ok_or_eyre("field not found")?;

    match move_value {
        MoveValue::Vector(move_vector) => {
            let structs = move_vector
                .iter()
                .map(|v| match v {
                    MoveValue::Struct(move_struct) => Ok(move_struct.clone()),
                    _ => bail!("expected struct"),
                })
                .collect::<Result<Vec<_>>>()?;

            Ok(structs)
        }
        _ => bail!("expected array"),
    }
}

pub fn extract_u128_from_move_struct(move_struct: &MoveStruct, field_name: &str) -> Result<u128> {
    let move_value = extract_field_from_move_struct(move_struct, field_name).ok_or_eyre("field not found")?;

    match move_value {
        MoveValue::U128(u) => Ok(*u),
        _ => bail!("expected u128"),
    }
}

pub fn extract_u64_from_move_struct(move_struct: &MoveStruct, field_name: &str) -> Result<u64> {
    let move_value = extract_field_from_move_struct(move_struct, field_name).ok_or_eyre("field not found")?;

    match move_value {
        MoveValue::U64(u) => Ok(*u),
        _ => bail!("expected u64"),
    }
}

pub fn extract_u32_from_move_struct(move_struct: &MoveStruct, field_name: &str) -> Result<u32> {
    let move_value = extract_field_from_move_struct(move_struct, field_name).ok_or_eyre("field not found")?;

    match move_value {
        MoveValue::U32(u) => Ok(*u),
        _ => bail!("expected u32"),
    }
}

pub fn extract_bool_from_move_struct(move_struct: &MoveStruct, field_name: &str) -> Result<bool> {
    let move_value = extract_field_from_move_struct(move_struct, field_name).ok_or_eyre("field not found")?;

    match move_value {
        MoveValue::Bool(b) => Ok(*b),
        _ => bail!("expected bool"),
    }
}

pub fn extract_u64_vec_from_move_struct(move_struct: &MoveStruct, field_name: &str) -> Result<Vec<u64>> {
    let move_value = extract_field_from_move_struct(move_struct, field_name).ok_or_eyre("field not found")?;

    match move_value {
        MoveValue::Vector(move_vector) => {
            let values = move_vector
                .iter()
                .map(|v| match v {
                    MoveValue::U64(u) => Ok(*u),
                    _ => bail!("expected u64"),
                })
                .collect::<Result<Vec<_>>>()?;

            Ok(values)
        }
        _ => bail!("expected vector"),
    }
}

pub fn extract_u128_vec_from_move_struct(move_struct: &MoveStruct, field_name: &str) -> Result<Vec<u128>> {
    let move_value = extract_field_from_move_struct(move_struct, field_name).ok_or_eyre("field not found")?;

    match move_value {
        MoveValue::Vector(move_vector) => {
            let values = move_vector
                .iter()
                .map(|v| match v {
                    MoveValue::U128(u) => Ok(*u),
                    _ => bail!("expected u128"),
                })
                .collect::<Result<Vec<_>>>()?;

            Ok(values)
        }
        _ => bail!("expected vector"),
    }
}

pub fn shared_obj_arg(obj: &Object, mutable: bool) -> ObjectArg {
    let initial_shared_version = match obj.owner() {
        Owner::Shared { initial_shared_version } => *initial_shared_version,
        _ => SequenceNumber::from_u64(0),
    };

    ObjectArg::SharedObject {
        id: obj.id(),
        initial_shared_version,
        mutable,
    }
}
