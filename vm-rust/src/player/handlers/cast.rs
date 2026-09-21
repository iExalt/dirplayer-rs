use crate::{
    director::lingo::datum::Datum,
    player::{session::ExecutionContext, DatumRef, ScriptError},
};

pub struct CastHandlers {}

impl CastHandlers {
    pub fn cast_lib(
        runtime: &mut ExecutionContext<'_>,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        let name_or_number = runtime.player.get_datum(&args[0]);
        let cast = match name_or_number {
            Datum::Int(n) => Some(runtime.player.movie.cast_manager.get_cast(*n as u32)?),
            Datum::String(s) => runtime.player.movie.cast_manager.get_cast_by_name(&s),
            _ => return Err(ScriptError::new("Invalid argument for castLib".to_owned())),
        };

        match cast {
            Some(c) => Ok(runtime.player.alloc_datum(Datum::CastLib(c.number))),
            None => Err(ScriptError::new("Cast not found".to_owned())),
        }
    }

    pub fn find_empty(
        runtime: &mut ExecutionContext<'_>,
        args: &Vec<DatumRef>,
    ) -> Result<DatumRef, ScriptError> {
        let member_ref = runtime.player.get_datum(&args[0]).to_member_ref()?;

        let (c_start, c_end) = match &runtime.player.movie.file {
            Some(file) => (file.config.min_member as u32, file.config.max_member as u32),
            None => return Err(ScriptError::new("findEmpty: no movie file loaded".to_owned())),
        };

        let cast_lib = if member_ref.cast_lib > 0 {
            member_ref.cast_lib as u32
        } else {
            1
        };
        let cast = runtime.player.movie.cast_manager.get_cast(cast_lib)?;

        let member_num = member_ref.cast_member as u32;
        if member_num > c_end {
            return Ok(runtime.player.alloc_datum(Datum::Int(member_num as i32)));
        }

        let start = if member_num > c_start { member_num } else { c_start };
        for slot in start..=c_end {
            if !cast.members.contains_key(&slot) {
                return Ok(runtime.player.alloc_datum(Datum::Int(slot as i32)));
            }
        }
        Ok(runtime.player.alloc_datum(Datum::Int(c_end as i32 + 1)))
    }
}
