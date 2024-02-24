use std::vec;

use proc_macro2::Span;
use syn::{punctuated::Punctuated, *};

pub struct GenericOptions<I: IntoIterator<Item = GenericArgument>> {
    pub leading_semi: bool,
    pub args: I,
}

pub trait PathConstructorExt: Sized {
    fn ident_segments_generic<
        S: ToString,
        L: IntoIterator<Item = S>,
        G: IntoIterator<Item = GenericArgument>,
    >(
        segments: L,
        generic: Option<GenericOptions<G>>,
    ) -> Self;

    fn ident_segments<S: ToString, L: IntoIterator<Item = S>>(segments: L) -> Self {
        Self::ident_segments_generic::<S, L, Vec<_>>(segments, None)
    }

    #[inline]
    fn ident<S: ToString>(ident: S) -> Self {
        Self::ident_segments([ident])
    }
}

impl PathConstructorExt for Path {
    fn ident_segments_generic<
        S: ToString,
        L: IntoIterator<Item = S>,
        G: IntoIterator<Item = GenericArgument>,
    >(
        segments: L,
        generics: Option<GenericOptions<G>>,
    ) -> Self {
        let mut segments: Vec<_> = segments
            .into_iter()
            .map(|it| {
                let name = it.to_string();
                PathSegment {
                    ident: Ident::new(&name, Span::call_site()),
                    arguments: PathArguments::None,
                }
            })
            .collect();

        if let (Some(generics), Some(last)) = (generics, segments.last_mut()) {
            last.arguments = PathArguments::AngleBracketed(AngleBracketedGenericArguments {
                colon2_token: if generics.leading_semi {
                    Some(Default::default())
                } else {
                    None
                },
                lt_token: Default::default(),
                args: Punctuated::from_iter(generics.args),
                gt_token: Default::default(),
            })
        }

        Path {
            leading_colon: None,
            segments: Punctuated::from_iter(segments),
        }
    }
}

impl PathConstructorExt for TypePath {
    #[inline]
    fn ident_segments_generic<
        S: ToString,
        L: IntoIterator<Item = S>,
        G: IntoIterator<Item = GenericArgument>,
    >(
        segments: L,
        generics: Option<GenericOptions<G>>,
    ) -> Self {
        TypePath {
            qself: None,
            path: Path::ident_segments_generic(segments, generics),
        }
    }
}

impl PathConstructorExt for ExprPath {
    #[inline]
    fn ident_segments_generic<
        S: ToString,
        L: IntoIterator<Item = S>,
        G: IntoIterator<Item = GenericArgument>,
    >(
        segments: L,
        generics: Option<GenericOptions<G>>,
    ) -> Self {
        ExprPath {
            attrs: vec![],
            qself: None,
            path: Path::ident_segments_generic(segments, generics),
        }
    }
}

impl PathConstructorExt for Expr {
    #[inline]
    fn ident_segments_generic<
        S: ToString,
        L: IntoIterator<Item = S>,
        G: IntoIterator<Item = GenericArgument>,
    >(
        segments: L,
        generics: Option<GenericOptions<G>>,
    ) -> Self {
        Expr::Path(ExprPath::ident_segments_generic(segments, generics))
    }
}

pub trait ListLikeExt<T> {
    fn pop_front(&mut self) -> Option<T>;
}

impl<T, P> ListLikeExt<T> for Punctuated<T, P>
where
    P: Default,
{
    fn pop_front(&mut self) -> Option<T> {
        let mut current = Punctuated::default();
        std::mem::swap(self, &mut current);
        let mut iter = current.into_iter();
        let result = iter.next();
        std::mem::swap(self, &mut Punctuated::from_iter(iter));
        result
    }
}

pub trait ErrorExt: Sized {
    fn from_many<E: IntoIterator<Item = Error>>(errors: E) -> Option<Self>;
}

impl ErrorExt for Error {
    fn from_many<E: IntoIterator<Item = Error>>(errors: E) -> Option<Self> {
        let mut iter = errors.into_iter();
        let mut result = match iter.next() {
            Some(it) => it,
            None => return None,
        };

        for other in iter {
            result.combine(other);
        }

        Some(result)
    }
}

const FULL_UPPER: &[&str] = &["xy", "xyz", "srgb", "xyzd50", "2d"];

pub fn snake_to_camel<S: ToString>(name: S) -> String {
    let name = name.to_string();
    let mut parts = name.split('_');
    let mut result = String::with_capacity(name.len());

    match parts.next() {
        Some(first) => result.push_str(first),
        None => return result,
    }

    for part in parts {
        if FULL_UPPER.contains(&part) {
            result.push_str(&part.to_uppercase());
        } else {
            let mut chars = part.chars();
            let mut name = match chars.next() {
                Some(it) => it.to_uppercase().to_string(),
                None => continue,
            };
            name.extend(chars);
            result.push_str(&name);
        }
    }

    result
}

pub fn ty_base_name(ty: &Type) -> Option<String> {
    let last = match ty {
        Type::Path(path) => path.path.segments.last()?,
        _ => return None,
    };
    Some(last.ident.to_string())
}

pub fn some_value(value: Expr) -> Expr {
    Expr::Call(ExprCall {
        attrs: vec![],
        func: Box::new(Expr::ident("Some")),
        paren_token: Default::default(),
        args: Punctuated::from_iter(std::iter::once(value)),
    })
}

pub fn cloned_value(value: Expr) -> Expr {
    Expr::MethodCall(ExprMethodCall {
        attrs: vec![],
        receiver: Box::new(value),
        dot_token: Default::default(),
        method: Ident::new("clone", Span::call_site()),
        turbofish: None,
        paren_token: Default::default(),
        args: Punctuated::new(),
    })
}
