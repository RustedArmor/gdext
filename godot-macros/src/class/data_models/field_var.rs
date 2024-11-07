/*
 * Copyright (c) godot-rust; Bromeon and contributors.
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};

use crate::class::{
    into_signature_info, make_existence_check, make_method_registration, Field, FieldHint,
    FuncDefinition,
};
use crate::util::{bail, KvParser};
use crate::{util, ParseResult};

/// Store info from `#[var]` attribute.
#[derive(Default, Clone, Debug)]
pub struct FieldVar {
    pub getter: GetterSetter,
    pub setter: GetterSetter,
    pub notify: Option<Ident>,
    pub hint: FieldHint,
    pub usage_flags: UsageFlags,
}

fn parse_notify(parser: &mut KvParser, key: &str) -> ParseResult<Option<Ident>> {
    match parser.handle_any(key) {
        // No `notify` argument
        None => Ok(None),
        Some(value) => match value {
            // `notify` without value is an error
            None => {
                bail!(parser.span(), "The correct syntax is 'notify = callback_fn'")
            },
            // `notify = expr`
            Some(value) => {
                match value.ident() {
                    Ok(ident) => Ok(Some(ident)),
                    Err(_) => bail!(parser.span(), "The correct syntax is 'notify = callback_fn'")
                }
            }
        }
    }
}

impl FieldVar {
    /// Parse a `#[var]` attribute to a `FieldVar` struct.
    ///
    /// Possible keys:
    /// - `get = expr`
    /// - `set = expr`
    /// - `hint = ident`
    /// - `hint_string = expr`
    /// - `usage_flags =
    pub(crate) fn new_from_kv(parser: &mut KvParser) -> ParseResult<Self> {
        let mut getter = GetterSetter::parse(parser, "get")?;
        let mut setter = GetterSetter::parse(parser, "set")?;
        let mut notify = parse_notify(parser, "notify")?;

        if getter.is_omitted() && setter.is_omitted() {
            getter = GetterSetter::Generated;
            setter = GetterSetter::Generated;
        }

        if notify.is_some() && !setter.is_generated() {
            return bail!(parser.span(), "When using 'notify', the property must also use an autogenerated 'set'");
        }

        let hint = parser.handle_ident("hint")?;

        let hint = if let Some(hint) = hint {
            let hint_string = parser.handle_expr("hint_string")?;

            FieldHint::new(hint, hint_string)
        } else {
            FieldHint::Inferred
        };

        let usage_flags = if let Some(mut parser) = parser.handle_array("usage_flags")? {
            let mut flags = Vec::new();

            while let Some(flag) = parser.next_ident()? {
                flags.push(flag)
            }

            parser.finish()?;

            UsageFlags::Custom(flags)
        } else {
            UsageFlags::Inferred
        };

        Ok(FieldVar {
            getter,
            setter,
            notify,
            hint,
            usage_flags,
        })
    }
}

#[derive(Default, Clone, Eq, PartialEq, Debug)]
pub enum GetterSetter {
    /// Getter/setter should be omitted, field is write/read only.
    Omitted,

    /// Trivial getter/setter should be autogenerated.
    #[default]
    Generated,

    /// Getter/setter is handwritten by the user, and here is its identifier.
    Custom(Ident),
}

impl GetterSetter {
    pub(super) fn parse(parser: &mut KvParser, key: &str) -> ParseResult<Self> {
        let getter_setter = match parser.handle_any(key) {
            // No `get` argument
            None => GetterSetter::Omitted,
            Some(value) => match value {
                // `get` without value
                None => GetterSetter::Generated,
                // `get = expr`
                Some(value) => GetterSetter::Custom(value.ident()?),
            },
        };

        Ok(getter_setter)
    }

    /// Returns the name, implementation, and export tokens for this `GetterSetter` declaration, for the
    /// given field and getter/setter kind.
    ///
    /// Returns `None` if no getter/setter should be created.
    pub(super) fn to_impl(
        &self,
        class_name: &Ident,
        kind: GetSet,
        notify: Option<Ident>,
        field: &Field,
    ) -> Option<GetterSetterImpl> {
        match self {
            GetterSetter::Omitted => None,
            GetterSetter::Generated => Some(GetterSetterImpl::from_generated_impl(
                class_name, kind, notify, field,
            )),
            GetterSetter::Custom(function_name) => {
                Some(GetterSetterImpl::from_custom_impl(function_name))
            }
        }
    }

    pub fn is_omitted(&self) -> bool {
        matches!(self, GetterSetter::Omitted)
    }

    pub fn is_generated(&self) -> bool {
        matches!(self, GetterSetter::Generated)
    }
}

/// Used to determine whether a [`GetterSetter`] is supposed to be a getter or setter.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum GetSet {
    Get,
    Set,
}

impl GetSet {
    pub fn prefix(&self) -> &'static str {
        match self {
            GetSet::Get => "get_",
            GetSet::Set => "set_",
        }
    }
}

#[derive(Clone, Debug)]
pub struct GetterSetterImpl {
    pub function_name: Ident,
    pub function_impl: TokenStream,
    pub export_token: TokenStream,
}

impl GetterSetterImpl {
    fn from_generated_impl(class_name: &Ident, kind: GetSet, notify: Option<Ident>, field: &Field) -> Self {
        let Field {
            name: field_name,
            ty: field_type,
            ..
        } = field;

        let function_name = format_ident!("{}{field_name}", kind.prefix());

        let signature;
        let function_body;

        match kind {
            GetSet::Get => {
                signature = quote! {
                    fn #function_name(&self) -> <#field_type as ::godot::meta::GodotConvert>::Via
                };
                function_body = quote! {
                    <#field_type as ::godot::register::property::Var>::get_property(&self.#field_name)
                };
            }
            GetSet::Set => {
                signature = quote! {
                    fn #function_name(&mut self, #field_name: <#field_type as ::godot::meta::GodotConvert>::Via)
                };

                let function_body_set = quote! {
                    <#field_type as ::godot::register::property::Var>::set_property(&mut self.#field_name, #field_name);
                };

                function_body = match notify {
                    Some(ident) => {
                        quote! {
                            let prev_value = self.#field_name;
                            #function_body_set
                            if prev_value != self.#field_name {
                                self.#ident();
                            }
                        }
                    },
                    None => function_body_set
                };
            }
        }

        let function_impl = quote! {
            pub #signature {
                #function_body
            }
        };

        let signature = util::parse_signature(signature);
        let export_token = make_method_registration(
            class_name,
            FuncDefinition {
                signature_info: into_signature_info(signature, class_name, false),
                // Since we're analyzing a struct's field, we don't have access to the corresponding get/set function's
                // external (non-#[func]) attributes. We have to assume the function exists and has the name the user
                // gave us, with the expected signature.
                // Ideally, we'd be able to place #[cfg_attr] on #[var(get)] and #[var(set)] to be able to match a
                // #[cfg()] (for instance) placed on the getter/setter function, but that is not currently supported.
                external_attributes: Vec::new(),
                registered_name: None,
                is_script_virtual: false,
                rpc_info: None,
            },
        );

        let export_token = export_token.expect("getter/setter generation should not fail");

        Self {
            function_name,
            function_impl,
            export_token,
        }
    }

    fn from_custom_impl(function_name: &Ident) -> Self {
        Self {
            function_name: function_name.clone(),
            function_impl: TokenStream::new(),
            export_token: make_existence_check(function_name),
        }
    }
}

#[derive(Default, Clone, Debug)]
pub enum UsageFlags {
    /// The usage flags should be inferred based on context.
    #[default]
    Inferred,

    /// The usage flags should be inferred based on context, such that they include export.
    InferredExport,

    /// Use a custom set of usage flags provided by the user.
    Custom(Vec<Ident>),
}

impl UsageFlags {
    pub fn is_inferred(&self) -> bool {
        matches!(self, Self::Inferred)
    }
}
