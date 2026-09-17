use fxhash::FxHashMap;
use log::debug;

use crate::{
    director::lingo::datum::Datum,
    player::{symbols::symbol_table::SymbolTable, DatumRef, ScriptError},
};
use crate::player::{DirPlayer, OwnerToken};

fn with_player<T>(player: &mut DirPlayer, callback: impl FnOnce(&mut DirPlayer) -> T) -> T {
    callback(player)
}

fn checked_player_datum<'a>(player: &'a DirPlayer, datum_ref: &DatumRef) -> Result<&'a Datum, ScriptError> {
    player.allocator.try_get_datum(datum_ref).ok_or_else(|| ScriptError::new_code(
        crate::player::ScriptErrorCode::InvalidReference,
        "foreign or stale FileIO argument".to_owned(),
    ))
}

/// FileIO historically reports -43 for a failed read open. Write/read-write
/// opens still produce an empty open buffer but leave lastError at zero.
pub(crate) fn remote_open_error(mode: i32) -> i32 {
    if mode == 1 { -43 } else { 0 }
}

/// Resolve a file path from the Lingo world (which may use movie_path_override)
/// to a relative filename that can be fetched from the real net_manager.base_path.
/// Returns (resolved_relative_name, real_base_url) or None if no override applies.
fn resolve_override_path(player: &DirPlayer, file_path: &str) -> Option<(String, String)> {
    {
        let override_base = &player.movie.base_path;
        let real_base = player.net_manager.base_path.as_ref().map(|u| u.to_string());

        if override_base.is_empty() || real_base.is_none() {
            return None;
        }
        let real_base = real_base.unwrap();

        // Normalize both paths for comparison (backslash → forward slash, case-insensitive on Windows)
        let norm_file = file_path.replace('\\', "/");
        let norm_override = override_base.replace('\\', "/");

        // Check if the file path starts with the override base path
        let norm_file_lower = norm_file.to_lowercase();
        let norm_override_lower = norm_override.to_lowercase();
        let prefix = if norm_override_lower.ends_with('/') {
            norm_override_lower.clone()
        } else {
            format!("{}/", norm_override_lower)
        };

        if norm_file_lower.starts_with(&prefix) {
            let relative = &norm_file[prefix.len()..];
            Some((relative.to_string(), real_base))
        } else {
            // Also try just the filename
            let file_name = norm_file.rsplit('/').next().unwrap_or(&norm_file);
            Some((file_name.to_string(), real_base))
        }
    }
}

/// FileIO Xtra instance — virtual in-memory file with read/write cursor.
pub struct FileIoXtraInstance {
    /// Current file name (set by fileName or openFile/createFile)
    pub file_name: String,
    /// In-memory file content
    pub data: Vec<u8>,
    /// Current read/write position
    pub position: usize,
    /// Whether the file is currently open
    pub is_open: bool,
    /// Last error code (0 = no error)
    pub last_error: i32,
    /// Filter mask for displayOpen/displaySave dialogs
    pub filter_mask: String,
    /// Newline conversion mode: 0=none, 1=platform
    pub newline_conversion: i32,
    pub generation: u64,
}

impl FileIoXtraInstance {
    pub fn new() -> Self {
        FileIoXtraInstance {
            file_name: String::new(),
            data: Vec::new(),
            position: 0,
            is_open: false,
            last_error: 0,
            filter_mask: String::new(),
            newline_conversion: 0,
            generation: 0,
        }
    }

    fn read_until(&mut self, delimiter: Option<u8>, skip_whitespace: bool) -> String {
        if !self.is_open || self.position >= self.data.len() {
            return String::new();
        }
        let start = if skip_whitespace {
            let mut s = self.position;
            while s < self.data.len() && (self.data[s] == b' ' || self.data[s] == b'\t') {
                s += 1;
            }
            s
        } else {
            self.position
        };
        let mut end = start;
        while end < self.data.len() {
            let b = self.data[end];
            if let Some(delim) = delimiter {
                if b == delim {
                    break;
                }
            }
            // Always stop on newlines for readLine/readToken/readWord
            if delimiter.is_some() && (b == b'\r' || b == b'\n') {
                break;
            }
            end += 1;
        }
        // UTF-8 strict first, Win-1252 fallback. See io::encoding.
        let result = crate::io::encoding::decode_text_auto(&self.data[start..end]);
        // Advance past delimiter/newline
        self.position = end;
        if self.position < self.data.len() {
            let b = self.data[self.position];
            if b == b'\r' || b == b'\n' || (delimiter.is_some() && Some(b) == delimiter) {
                self.position += 1;
                // Handle \r\n pair
                if b == b'\r'
                    && self.position < self.data.len()
                    && self.data[self.position] == b'\n'
                {
                    self.position += 1;
                }
            }
        }
        result
    }
}

pub struct FileIoXtraManager {
    pub instances: FxHashMap<u32, FileIoXtraInstance>,
    pub instance_counter: u32,
    pub generation_counter: u64,
    pub owner: OwnerToken,
    /// Simple virtual filesystem: file_name -> data
    pub virtual_fs: FxHashMap<String, Vec<u8>>,
}

impl FileIoXtraManager {
    pub fn new() -> Self {
        Self::new_with_owner(OwnerToken::transitional())
    }

    pub fn new_with_owner(owner: OwnerToken) -> Self {
        FileIoXtraManager {
            instances: FxHashMap::default(),
            instance_counter: 0,
            generation_counter: 0,
            owner,
            virtual_fs: FxHashMap::default(),
        }
    }

    pub(crate) fn rebind_owner(&mut self, owner: OwnerToken) {
        self.owner = owner;
    }

    pub(crate) fn reset(&mut self) {
        self.instances.clear();
        self.instance_counter = 0;
    }

    pub(crate) fn create_instance_explicit(&mut self, _args: &[DatumRef]) -> Result<u32, ScriptError> {
        if !self.owner.is_arena_live() {
            return Err(ScriptError::new_code(
                crate::player::ScriptErrorCode::Abort,
                "FileIO owner was retired".to_owned(),
            ));
        }
        self.instance_counter = self
            .instance_counter
            .checked_add(1)
            .ok_or_else(|| ScriptError::new("FileIO instance id exhausted".to_owned()))?;
        self.generation_counter = self
            .generation_counter
            .checked_add(1)
            .ok_or_else(|| ScriptError::new("FileIO instance generation exhausted".to_owned()))?;
        let generation = self.generation_counter;
        let mut instance = FileIoXtraInstance::new();
        instance.generation = generation;
        self.instances
            .insert(self.instance_counter, instance);
        Ok(self.instance_counter)
    }

    pub(crate) fn instance_generation(&self, instance_id: u32) -> Option<u64> {
        self.instances.get(&instance_id).map(|instance| instance.generation)
    }

    /// Prepare openFile without retaining VM arena references across the
    /// network wait. Virtual files stay synchronous; a missing read file is
    /// represented by an owner/generation-qualified pending request.
    pub(crate) fn call_instance_handler_pending_explicit(
        &mut self,
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        instance_id: u32,
        handler_name: &str,
        args: &[DatumRef],
    ) -> Result<super::manager::XtraPendingOrValue, ScriptError> {
        if !self.owner.same_identity(&player.owner) || !self.owner.is_arena_live() {
            return Err(ScriptError::new_code(
                crate::player::ScriptErrorCode::Abort,
                "FileIO owner was retired".to_owned(),
            ));
        }
        let handler = handler_name.to_ascii_lowercase();
        if handler != "openfile" {
            return self
                .call_instance_handler_explicit(player, symbols, instance_id, handler_name, args)
                .map(super::manager::XtraPendingOrValue::Value);
        }
        let file_name = checked_player_datum(
            player,
            args.first().ok_or_else(|| ScriptError::new("openFile requires a file name".to_owned()))?,
        )?
        .string_value(symbols)?;
        let mode = args
            .get(1)
            .map(|arg| checked_player_datum(player, arg).and_then(|datum| datum.int_value()))
            .transpose()?
            .unwrap_or(1);
        let relative_name = resolve_override_path(player, &file_name)
            .map(|(relative, _)| relative)
            .unwrap_or_else(|| file_name.rsplit(['\\', '/']).next().unwrap_or(&file_name).to_owned());
        let generation = self.instance_generation(instance_id).ok_or_else(|| {
            ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                format!("FileIO instance #{} not found", instance_id),
            )
        })?;
        if self.virtual_fs.contains_key(&file_name)
            || self.virtual_fs.contains_key(&relative_name)
        {
            return self
                .call_instance_handler_explicit(player, symbols, instance_id, handler_name, args)
                .map(super::manager::XtraPendingOrValue::Value);
        }
        // Allocate the task while the invocation owns the player borrow. The
        // NetTask retains the resolved URL and the current base-path override;
        // the later pending executor must not resolve the name again.
        let prepared = player
            .net_manager
            .prepare_net_thing(relative_name.clone());
        Ok(super::manager::XtraPendingOrValue::Pending(
            super::manager::XtraPendingIntent::FileIoOpen(
                super::manager::FileIoOpenRequest {
                    owner: self.owner.clone(),
                    instance_id,
                    generation,
                    file_name,
                    relative_name,
                    prepared,
                    mode,
                },
            ),
        ))
    }

    pub(crate) fn call_instance_handler_explicit(
        &mut self,
        player: &mut DirPlayer,
        symbols: &mut SymbolTable,
        instance_id: u32,
        handler_name: &str,
        args: &[DatumRef],
    ) -> Result<DatumRef, ScriptError> {
        let manager = self;
        if !manager.owner.same_identity(&player.owner) || !manager.owner.is_arena_live() {
            return Err(ScriptError::new_code(
                crate::player::ScriptErrorCode::Abort,
                "FileIO owner was retired".to_owned(),
            ));
        }
        if !manager.instances.contains_key(&instance_id) {
            return Err(ScriptError::new_code(
                crate::player::ScriptErrorCode::InvalidReference,
                format!("FileIO instance #{} not found", instance_id),
            ));
        }
        let handler = handler_name.to_lowercase();

        match handler.as_str() {
            "openfile" => {
                let file_name = with_player(player, |player| {
                    let arg = args.first().ok_or_else(|| ScriptError::new("openFile requires a file name".to_owned()))?;
                    checked_player_datum(player, arg)?.string_value(symbols)
                })?;
                let mode = if args.len() > 1 {
                    with_player(player, |player| checked_player_datum(player, &args[1])?.int_value())?
                } else { 1 };
                let relative_name = resolve_override_path(player, &file_name)
                    .map(|(relative, _)| relative)
                    .unwrap_or_else(|| file_name.rsplit(['\\', '/']).next().unwrap_or(&file_name).to_owned());
                let data = manager.virtual_fs.get(&file_name).cloned()
                    .or_else(|| manager.virtual_fs.get(&relative_name).cloned());
                let instance = manager.instances.get_mut(&instance_id).expect("validated FileIO instance");
                instance.file_name = file_name;
                instance.position = 0;
                instance.last_error = if data.is_some() || mode != 1 { 0 } else { -43 };
                instance.data = data.unwrap_or_default();
                instance.is_open = true;
                with_player(player, |player| Ok(player.alloc_datum(Datum::Void)))
            }
            "createfile" => {
                let file_name =
                    with_player(player, |player| checked_player_datum(player, &args[0])?.string_value(symbols))?;
                let instance = manager.instances.get_mut(&instance_id).unwrap();
                instance.file_name = file_name;
                instance.data = Vec::new();
                instance.position = 0;
                instance.is_open = true;
                instance.last_error = 0;

                with_player(player, |player| Ok(player.alloc_datum(Datum::Void)))
            }
            "closefile" => {
                let instance = manager.instances.get_mut(&instance_id).unwrap();
                if instance.is_open && !instance.file_name.is_empty() {
                    // Persist to virtual filesystem
                    manager
                        .virtual_fs
                        .insert(instance.file_name.clone(), instance.data.clone());
                    // Re-borrow instance after virtual_fs insert
                    let instance = manager.instances.get_mut(&instance_id).unwrap();
                    instance.is_open = false;
                }
                Ok(DatumRef::Void)
            }
            "delete" => {
                let instance = manager.instances.get_mut(&instance_id).unwrap();
                if !instance.file_name.is_empty() {
                    manager.virtual_fs.remove(&instance.file_name.clone());
                }
                Ok(DatumRef::Void)
            }

            // -- Read operations --
            "readfile" => {
                let instance = manager.instances.get_mut(&instance_id).unwrap();
                let result = if instance.is_open {
                    crate::io::encoding::decode_text_auto(&instance.data[instance.position..])
                } else {
                    instance.last_error = -1;
                    String::new()
                };
                instance.position = instance.data.len();
                with_player(player, |player| Ok(player.alloc_datum(Datum::String(result))))
            }
            "readline" => {
                let instance = manager.instances.get_mut(&instance_id).unwrap();
                let line = instance.read_until(None, false);
                with_player(player, |player| Ok(player.alloc_datum(Datum::String(line))))
            }
            "readchar" => {
                let instance = manager.instances.get_mut(&instance_id).unwrap();
                let ch = if instance.is_open && instance.position < instance.data.len() {
                    let c = instance.data[instance.position] as char;
                    instance.position += 1;
                    c.to_string()
                } else {
                    String::new()
                };
                with_player(player, |player| Ok(player.alloc_datum(Datum::String(ch))))
            }
            "readword" => {
                let instance = manager.instances.get_mut(&instance_id).unwrap();
                let word = instance.read_until(Some(b' '), true);
                with_player(player, |player| Ok(player.alloc_datum(Datum::String(word))))
            }
            "readtoken" => {
                let instance = manager.instances.get_mut(&instance_id).unwrap();
                // readToken reads until the next delimiter specified by args
                let (skip_str, break_str) = with_player(player, |player| {
                    let s = if args.len() > 0 {
                        checked_player_datum(player, &args[0])?
                            .string_value(symbols)
                            .unwrap_or_default()
                    } else {
                        " \t".to_string()
                    };
                    let b = if args.len() > 1 {
                        checked_player_datum(player, &args[1])?
                            .string_value(symbols)
                            .unwrap_or_default()
                    } else {
                        "\r\n".to_string()
                    };
                    Ok::<_, ScriptError>((s, b))
                })?;
                // Skip leading skip chars
                while instance.position < instance.data.len() {
                    let ch = instance.data[instance.position] as char;
                    if skip_str.contains(ch) {
                        instance.position += 1;
                    } else {
                        break;
                    }
                }
                // Read until break char
                let start = instance.position;
                while instance.position < instance.data.len() {
                    let ch = instance.data[instance.position] as char;
                    if break_str.contains(ch) || skip_str.contains(ch) {
                        break;
                    }
                    instance.position += 1;
                }
                let token =
                    crate::io::encoding::decode_text_auto(&instance.data[start..instance.position]);
                with_player(player, |player| Ok(player.alloc_datum(Datum::String(token))))
            }

            // -- Write operations --
            "writestring" => {
                let text =
                    with_player(player, |player| checked_player_datum(player, &args[0])?.string_value(symbols))?;
                let instance = manager.instances.get_mut(&instance_id).unwrap();
                if instance.is_open {
                    let bytes = text.as_bytes();
                    // Insert at position (overwrite or extend)
                    if instance.position >= instance.data.len() {
                        instance.data.extend_from_slice(bytes);
                    } else {
                        let end = (instance.position + bytes.len()).min(instance.data.len());
                        let overwrite_len = end - instance.position;
                        instance.data[instance.position..end]
                            .copy_from_slice(&bytes[..overwrite_len]);
                        if bytes.len() > overwrite_len {
                            instance.data.extend_from_slice(&bytes[overwrite_len..]);
                        }
                    }
                    instance.position += bytes.len();
                    instance.last_error = 0;
                } else {
                    instance.last_error = -1;
                }
                Ok(DatumRef::Void)
            }
            "writechar" => {
                let ch =
                    with_player(player, |player| checked_player_datum(player, &args[0])?.string_value(symbols))?;
                let instance = manager.instances.get_mut(&instance_id).unwrap();
                if instance.is_open && !ch.is_empty() {
                    let byte = ch.as_bytes()[0];
                    if instance.position >= instance.data.len() {
                        instance.data.push(byte);
                    } else {
                        instance.data[instance.position] = byte;
                    }
                    instance.position += 1;
                }
                Ok(DatumRef::Void)
            }
            "writereturn" => {
                let instance = manager.instances.get_mut(&instance_id).unwrap();
                if instance.is_open {
                    if instance.position >= instance.data.len() {
                        instance.data.push(b'\r');
                    } else {
                        instance.data.insert(instance.position, b'\r');
                    }
                    instance.position += 1;
                }
                Ok(DatumRef::Void)
            }

            // -- Position/length --
            "getlength" => {
                let instance = manager.instances.get(&instance_id).unwrap();
                with_player(player, |player| {
                    Ok(player.alloc_datum(Datum::Int(instance.data.len() as i32)))
                })
            }
            "getposition" => {
                let instance = manager.instances.get(&instance_id).unwrap();
                with_player(player, |player| {
                    Ok(player.alloc_datum(Datum::Int(instance.position as i32)))
                })
            }
            "setposition" => {
                let pos = with_player(player, |player| checked_player_datum(player, &args[0])?.int_value())?;
                let instance = manager.instances.get_mut(&instance_id).unwrap();
                instance.position = (pos as usize).min(instance.data.len());
                Ok(DatumRef::Void)
            }

            // -- Properties --
            "filename" => {
                if !args.is_empty() {
                    // setter
                    let name = with_player(player, |player| {
                        checked_player_datum(player, &args[0])?.string_value(symbols)
                    })?;
                    let instance = manager.instances.get_mut(&instance_id).unwrap();
                    instance.file_name = name;
                    Ok(DatumRef::Void)
                } else {
                    // getter
                    let instance = manager.instances.get(&instance_id).unwrap();
                    let name = instance.file_name.clone();
                    with_player(player, |player| Ok(player.alloc_datum(Datum::String(name))))
                }
            }
            "status" => {
                let instance = manager.instances.get(&instance_id).unwrap();
                let status = instance.last_error;
                with_player(player, |player| Ok(player.alloc_datum(Datum::Int(status))))
            }
            "error" => {
                let instance = manager.instances.get(&instance_id).unwrap();
                let msg = match instance.last_error {
                    0 => "OK",
                    -43 => "File not found",
                    -1 => "File not open",
                    _ => "Unknown error",
                };
                with_player(player, |player| Ok(player.alloc_datum(Datum::String(msg.to_string()))))
            }
            "version" => {
                with_player(player, 
                    |player| Ok(player.alloc_datum(Datum::String("1.5".to_string()))),
                )
            }
            "setfiltermask" => {
                let mask =
                    with_player(player, |player| checked_player_datum(player, &args[0])?.string_value(symbols))?;
                let instance = manager.instances.get_mut(&instance_id).unwrap();
                instance.filter_mask = mask;
                Ok(DatumRef::Void)
            }
            "setnewlineconversion" => {
                let mode = with_player(player, |player| checked_player_datum(player, &args[0])?.int_value())?;
                let instance = manager.instances.get_mut(&instance_id).unwrap();
                instance.newline_conversion = mode;
                Ok(DatumRef::Void)
            }
            "getosdirectory" => {
                with_player(player, |player| Ok(player.alloc_datum(Datum::String("/".to_string()))))
            }
            "getfinderinfo" | "setfinderinfo" => {
                // Finder info is Mac-specific, return empty/no-op
                with_player(player, |player| Ok(player.alloc_datum(Datum::String(String::new()))))
            }

            // -- Dialog stubs (sync fallback) --
            "displayopen" | "displaysave" => {
                debug!("FileIO.{}(): sync fallback, returning empty", handler_name);
                with_player(player, |player| Ok(player.alloc_datum(Datum::String(String::new()))))
            }

            // -- put interface --
            "interface" => {
                let interface_str = [
                    "-- xtra FileIO",
                    "new object me",
                    "createFile string fileName -- creates file",
                    "openFile string fileName, int mode -- opens file (1=read,2=write,0=rw)",
                    "closeFile object me -- close file",
                    "readFile object me -- read entire file",
                    "readLine object me -- read a line",
                    "readChar object me -- read one character",
                    "readWord object me -- read a word",
                    "readToken string skipChars, string breakChars -- read a token",
                    "writeString string text -- write text",
                    "writeChar string ch -- write one character",
                    "writeReturn object me -- write a carriage return",
                    "fileName object me -- get or set file name",
                    "getLength object me -- get file length",
                    "getPosition object me -- get cursor position",
                    "setPosition int pos -- set cursor position",
                    "delete object me -- delete the file",
                    "status object me -- get error code",
                    "error object me -- get error message",
                    "version object me -- get xtra version",
                    "setFilterMask string mask -- set file dialog filter",
                    "setNewlineConversion int mode -- set newline conversion",
                    "getOSDirectory -- get OS directory path",
                    "displayOpen -- show open file dialog",
                    "displaySave string title, string name -- show save file dialog",
                ]
                .join("\n");
                with_player(player, |player| Ok(player.alloc_datum(Datum::String(interface_str))))
            }

            _ => Err(ScriptError::new(format!(
                "No handler {} found for FileIO xtra instance #{}",
                handler_name, instance_id
            ))),
        }
    }
}
