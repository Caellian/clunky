use syn::{
    parse::{Parse, ParseBuffer, ParseStream},
    punctuated::Punctuated,
    spanned::Spanned,
    *,
};

pub struct ConfigEntry {
    pub name: Ident,
    pub value: Option<(Token![:], Expr)>,
}

impl ConfigEntry {
    pub fn value(&self) -> Result<&Expr> {
        self.value
            .as_ref()
            .map(|it| &it.1)
            .ok_or_else(|| Error::new(self.name.span(), format!("missing value for {}", self.name)))
    }
}

impl Parse for ConfigEntry {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut result = ConfigEntry {
            name: input.parse()?,
            value: None,
        };

        if input.peek(Token![:]) {
            result.value = Some((input.parse()?, input.parse()?));
        }

        Ok(result)
    }
}

#[derive(Default)]
pub struct FunctionOptions {
    pub is_mut: bool,
}

impl FunctionOptions {
    fn parse(input: ParseBuffer<'_>) -> Result<Self> {
        let mut result: FunctionOptions = FunctionOptions::default();

        if input.is_empty() {
            return Ok(result);
        }

        const FN_ARGS: &str = "supported function options: 'mut'";

        if input.peek(token::Mut) {
            match input.parse::<token::Mut>() {
                Ok(_) => {
                    result.is_mut = true;
                }
                Err(err) => return Err(Error::new(err.span(), FN_ARGS)),
            }
        } else {
            return Err(input.error(FN_ARGS));
        }

        Ok(result)
    }
}

#[derive(Default)]
pub struct EntryOptions {
    pub function: Option<FunctionOptions>,
    pub metamethod: Option<Path>,
    pub skip: bool,
    pub constructor: bool,
    pub rename: Option<String>,
}

impl Parse for EntryOptions {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut options = EntryOptions::default();

        if input.is_empty() {
            return Ok(options);
        }

        let args = Punctuated::<ConfigEntry, token::Comma>::parse_separated_nonempty(input)?;

        for it in args {
            let name = it.name.to_string();

            match name.as_str() {
                "function" => {
                    if let Ok(value) = it.value() {
                        let inner;
                        syn::parenthesized!(inner in input);
                        options.function = Some(FunctionOptions::parse(inner)?);
                    } else {
                        options.function = Some(FunctionOptions::default());
                    }
                }
                "skip" => {
                    options.skip = true;
                }
                "rename" => match it.value()? {
                    Expr::Path(ExprPath {
                        qself: None, path, ..
                    }) => {
                        if path.segments.len() > 1 {
                            return Err(Error::new_spanned(
                                path,
                                "too many path segments; rename accepts only a name",
                            ));
                        }

                        let last = path.segments.last().expect("empty path segments");
                        if !last.arguments.is_none() {
                            return Err(Error::new_spanned(path, "path arguments not supported"));
                        }
                        options.rename = Some(last.ident.to_string());
                    }
                    Expr::Lit(ExprLit {
                        lit: Lit::Str(name),
                        ..
                    }) => {
                        options.rename = Some(name.value());
                    }
                    other => {
                        return Err(Error::new(
                            other.span(),
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

impl EntryOptions {
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

        Some(syn::parse::<EntryOptions>(list.into()))
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
                    format!(
                        "expecting comma separated 'key: value' pairs; {}",
                        reason.to_string()
                    ),
                )
            })?;

        for e in args {
            let name = e.name.to_string();
            match name.as_str() {
                "lua_name" => {
                    let value = e.value()?;

                    options.lua_name = Some(
                        if let Expr::Path(ExprPath {
                            qself: None, path, ..
                        }) = value
                        {
                            if path.segments.len() > 1 {
                                return Err(Error::new_spanned(
                                    path,
                                    "too many path segments; lua_name accepts only a name",
                                ));
                            }

                            let last = path.segments.last().expect("empty path segments");
                            if !last.arguments.is_none() {
                                return Err(Error::new_spanned(
                                    path,
                                    "path arguments not supported",
                                ));
                            }

                            last.ident.to_string()
                        } else if let Expr::Lit(ExprLit {
                            lit: Lit::Str(name),
                            ..
                        }) = value
                        {
                            name.value()
                        } else {
                            return Err(Error::new_spanned(value, "lua_name expects a name"));
                        },
                    );
                }
                other => {
                    return Err(Error::new_spanned(
                        e.name,
                        format!("unknown option: {other}"),
                    ))
                }
            }
        }

        Ok(options)
    }
}
