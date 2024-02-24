use quote::ToTokens;
use syn::{
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
    *,
};

pub enum DiscreteValue {
    Ident(Ident),
    Lit(Lit),
}

impl Parse for DiscreteValue {
    fn parse(input: ParseStream) -> Result<Self> {
        if input.peek(syn::Ident) {
            Ok(DiscreteValue::Ident(input.parse()?))
        } else if input.peek(syn::Lit) {
            Ok(DiscreteValue::Lit(input.parse()?))
        } else {
            Err(input.error("expected an ident or literal"))
        }
    }
}

impl ToTokens for DiscreteValue {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        match self {
            DiscreteValue::Ident(ident) => ident.to_tokens(tokens),
            DiscreteValue::Lit(lit) => lit.to_tokens(tokens),
        }
    }
}

pub enum EntryValue {
    None,
    Colon(Token![:], DiscreteValue),
    Eq(Token![=], DiscreteValue),
    Paren(token::Paren, Punctuated<ConfigEntry, Token![,]>),
}

impl Parse for EntryValue {
    fn parse(input: ParseStream) -> Result<Self> {
        if input.peek(Token![:]) {
            Ok(EntryValue::Colon(input.parse()?, input.parse()?))
        } else if input.peek(Token![=]) {
            Ok(EntryValue::Eq(input.parse()?, input.parse()?))
        } else if input.peek(token::Paren) {
            let content;
            let paren = syn::parenthesized!(content in input);
            let values = Punctuated::<ConfigEntry, Token![,]>::parse_separated_nonempty(&content)?;
            Ok(EntryValue::Paren(paren, values))
        } else if input.is_empty() || input.peek(Token![,]) {
            Ok(EntryValue::None)
        } else {
            Err(input.error("expected an entry value"))
        }
    }
}

impl ToTokens for EntryValue {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        match self {
            EntryValue::None => {}
            EntryValue::Colon(a, b) => {
                a.to_tokens(tokens);
                b.to_tokens(tokens);
            }
            EntryValue::Eq(a, b) => {
                a.to_tokens(tokens);
                b.to_tokens(tokens);
            }
            EntryValue::Paren(a, b) => a.surround(tokens, |tokens| {
                for it in b.pairs() {
                    it.value().to_tokens(tokens);
                    if let Some(punct) = it.punct() {
                        punct.to_tokens(tokens);
                    }
                }
            }),
        }
    }
}

impl EntryValue {
    pub fn is_none(&self) -> bool {
        matches!(self, EntryValue::None)
    }
    pub fn single(&self) -> Option<&DiscreteValue> {
        match self {
            EntryValue::Colon(_, value) | EntryValue::Eq(_, value) => Some(value),
            _ => None,
        }
    }
    pub fn many(&self) -> Option<impl Iterator<Item = &ConfigEntry> + '_> {
        match self {
            EntryValue::Paren(_, items) => Some(items.iter()),
            _ => None,
        }
    }
}

pub struct ConfigEntry {
    pub name: Ident,
    pub value: EntryValue,
}

impl Parse for ConfigEntry {
    fn parse(input: ParseStream) -> Result<Self> {
        Ok(ConfigEntry {
            name: input.parse()?,
            value: input.parse()?,
        })
    }
}

impl ToTokens for ConfigEntry {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        self.name.to_tokens(tokens);
        self.value.to_tokens(tokens);
    }
}

#[derive(Default)]
pub struct FunctionOptions {
    pub is_mut: bool,
}

impl FunctionOptions {
    fn new<'a, I: IntoIterator<Item = &'a ConfigEntry>>(options: I) -> Result<Self> {
        let mut result = Self::default();
        for it in options.into_iter() {
            let name = it.name.to_string();

            match name.as_str() {
                "mut" => {
                    if it.value.is_none() {
                        result.is_mut = true;
                    } else {
                        return Err(Error::new_spanned(
                            &it.value,
                            "'mut' option doesn't accept any values",
                        ));
                    }
                }
                other => {
                    return Err(Error::new_spanned(
                        &it.name,
                        format!("unknown option: {other}"),
                    ))
                }
            }
        }
        Ok(result)
    }
}

#[derive(Default)]
pub struct ItemOptions {
    pub function: Option<FunctionOptions>,
    pub metamethod: Option<Path>,
    pub skip: bool,
    pub constructor: bool,
    pub rename: Option<String>,
}

impl Parse for ItemOptions {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut options = ItemOptions::default();

        if input.is_empty() {
            return Ok(options);
        }

        let args = Punctuated::<ConfigEntry, token::Comma>::parse_separated_nonempty(input)?;

        for it in args {
            let name = it.name.to_string();

            match name.as_str() {
                "function" => {
                    if let Some(value) = it.value.many() {
                        options.function = Some(FunctionOptions::new(value)?);
                    } else if it.value.is_none() {
                        options.function = Some(FunctionOptions::default());
                    } else {
                        return Err(Error::new_spanned(
                            &it.value,
                            "expected property list or nothing",
                        ));
                    }
                }
                "skip" => {
                    options.skip = true;
                }
                "rename" => match it.value.single() {
                    Some(DiscreteValue::Ident(ident)) => {
                        options.rename = Some(ident.to_string());
                    }
                    Some(DiscreteValue::Lit(Lit::Str(name))) => {
                        options.rename = Some(name.value());
                    }
                    _ => {
                        return Err(Error::new_spanned(
                            it.value,
                            "rename value must be an ident or string literal",
                        ));
                    }
                },
                "constructor" => {
                    options.constructor = true;
                }
                other => {
                    return Err(Error::new(
                        it.name.span(),
                        format!("unknown option: {other}"),
                    ));
                }
            }
        }

        Ok(options)
    }
}

impl ItemOptions {
    pub fn check(meta: &Meta) -> bool {
        match meta {
            Meta::Path(path) => {
                path.segments.len() == 1
                    && path.segments.first().map(|it| it.ident == "lua") == Some(true)
            }
            Meta::List(list) => {
                list.path.segments.len() == 1
                    && list.path.segments.first().map(|it| it.ident == "lua") == Some(true)
            }
            _ => false,
        }
    }

    pub fn from_meta(meta: &Meta) -> Option<Result<Self>> {
        let list = match meta {
            Meta::Path(path)
                if path.segments.len() == 1 && path.segments.first()?.ident == "lua" =>
            {
                return Some(Err(Error::new_spanned(path, "expected list arguments")));
            }
            Meta::List(list)
                if list.path.segments.len() == 1 && list.path.segments.first()?.ident == "lua" =>
            {
                list.tokens.clone()
            }
            _ => return None,
        };

        Some(syn::parse::<ItemOptions>(list.into()))
    }
}

#[derive(Default)]
pub struct AttributeOptions {
    pub lua_name: Option<String>,
}

impl Parse for AttributeOptions {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut options = AttributeOptions::default();

        if input.is_empty() {
            return Ok(options);
        }

        let args = Punctuated::<ConfigEntry, token::Comma>::parse_separated_nonempty(input)
            .map_err(|reason| {
                Error::new(
                    reason.span(),
                    format!("expecting comma separated 'key: value' pairs; {reason}"),
                )
            })?;

        for it in args {
            let name = it.name.to_string();
            match name.as_str() {
                "lua_name" => match it.value.single() {
                    Some(DiscreteValue::Ident(ident)) => {
                        options.lua_name = Some(ident.to_string());
                    }
                    Some(DiscreteValue::Lit(Lit::Str(name))) => {
                        options.lua_name = Some(name.value());
                    }
                    _ => {
                        return Err(Error::new_spanned(it.value, "lua_name expects a name"));
                    }
                },
                other => {
                    return Err(Error::new_spanned(
                        it.name,
                        format!("unknown option: {other}"),
                    ))
                }
            }
        }

        Ok(options)
    }
}
