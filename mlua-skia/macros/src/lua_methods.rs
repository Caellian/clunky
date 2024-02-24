use proc_macro2::Span;
use syn::{
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
    spanned::Spanned,
    visit_mut::VisitMut,
    *,
};

use crate::{
    options::{AttributeOptions, ItemOptions},
    util::*,
};

enum SignatureKind {
    Method { recv: Receiver },
    Function { mutability: bool },
}

impl Default for SignatureKind {
    fn default() -> Self {
        SignatureKind::Function { mutability: false }
    }
}

struct MethodSignature {
    asyncness: Option<Token![async]>,
    is_meta: bool,
    kind: SignatureKind,

    options: ItemOptions,
    lua_ctx: Option<(Lifetime, Ident)>,
    name: Ident,
    inputs: Punctuated<FnArg, Token![,]>,
}

impl MethodSignature {
    pub fn lua_name(&self) -> String {
        self.options
            .rename
            .clone()
            .unwrap_or_else(|| snake_to_camel(&self.name))
    }

    pub fn register_with(&self) -> Ident {
        let mut result = String::with_capacity(25);
        result.push_str("add");

        if self.asyncness.is_some() {
            result.push_str("_async");
        }
        if self.is_meta {
            result.push_str("_meta");
        }
        match &self.kind {
            SignatureKind::Method { recv } => {
                result.push_str("_method");
                if recv.mutability.is_some() {
                    result.push_str("_mut");
                }
            }
            SignatureKind::Function { mutability } => {
                result.push_str("_function");
                if *mutability {
                    result.push_str("_mut");
                }
            }
        }

        Ident::new(&result, Span::call_site())
    }
}

// TODO: Gen rust impl code
static METAMETHODS: &[&str] = &[
    "__index",
    "__newindex",
    "__call",
    "__concat",
    "__unm",
    "__add",
    "__sub",
    "__mul",
    "__div",
    "__idiv",
    "__mod",
    "__pow",
    "__tostring",
    "__metatable",
    "__eq",
    "__lt",
    "__le",
    "__mode",
    "__len",
    "__iter",
];

const SELF_MAPPED: &str = "__cb_this";
const CTX_ERASED: &str = "__lua_ctx";
const ARGS_MAPPED: &str = "__lua_cb_args";
const REF_SUFFIX: &str = "_ud_ref";

fn is_path_lua(path: &Path) -> bool {
    if path.segments.iter().any(|it| !it.arguments.is_none()) {
        return false;
    }
    let name = path
        .segments
        .iter()
        .map(|it| it.ident.to_string())
        .collect::<Vec<_>>()
        .join("::");

    matches!(name.as_str(), "mlua::Lua" | "Lua" | "LuaContext")
}

fn lua_ctx_name(arg: &FnArg) -> Option<(Lifetime, Ident)> {
    let arg = match arg {
        FnArg::Typed(it) => it,
        FnArg::Receiver(_) => return None,
    };

    let arg_ty = match arg.ty.as_ref() {
        Type::Reference(it) => it,
        _ => return None,
    };

    let arg_lt = arg_ty.lifetime.clone()?;

    match arg_ty.elem.as_ref() {
        Type::Path(TypePath { qself: None, path }) => {
            if !is_path_lua(path) {
                return None;
            }
        }
        _ => return None,
    }

    let arg_ident = match arg.pat.as_ref() {
        Pat::Ident(it) => it,
        _ => return None,
    };

    Some((arg_lt, arg_ident.ident.clone()))
}

impl MethodSignature {
    fn new(function_impl: &ImplItemFn) -> Result<Self> {
        let sig = &function_impl.sig;
        let name = sig.ident.clone();
        let name_str = name.to_string();
        let is_meta = METAMETHODS.contains(&name_str.as_str());

        let mut inputs = sig.inputs.clone();
        let mut kind = None;

        if let Some(FnArg::Receiver(recv)) = inputs.first().cloned() {
            kind = Some(SignatureKind::Method { recv });
            inputs.pop_front();
        }

        let lua_ctx = if let Some(first) = inputs.first().cloned() {
            let ctx = lua_ctx_name(&first);
            if ctx.is_some() {
                inputs.pop_front();
            }
            ctx
        } else {
            None
        };

        let mut options = ItemOptions::default();
        for attr in &function_impl.attrs {
            if let Some(o) = ItemOptions::from_meta(&attr.meta) {
                options = o?;
                break;
            }
        }

        if let Some(function) = &options.function {
            kind = Some(SignatureKind::Function {
                mutability: function.is_mut,
            });
        }

        let kind = kind.unwrap_or_default();

        if let SignatureKind::Function { mutability: true } = kind {
            if let Some(asyncness) = sig.asyncness {
                if is_meta {
                    return Err(Error::new_spanned(
                        asyncness,
                        "mutable async meta functions not supported",
                    ));
                }
            }
        }

        Ok(MethodSignature {
            asyncness: sig.asyncness,
            is_meta,
            kind,
            options,
            lua_ctx,
            inputs,
            name,
        })
    }

    fn block_setup_statements(&self, ctx_name: &str, skip_table: bool) -> Result<Vec<Stmt>> {
        let mut result = Vec::with_capacity(3);

        let name = snake_to_camel(&self.name);

        let init = Expr::Call(ExprCall {
            attrs: vec![],
            func: Box::new(Expr::ident_segments([
                "crate",
                "lua",
                "FromArgs",
                "from_arguments",
            ])),
            paren_token: Default::default(),
            args: Punctuated::from_iter([
                Expr::ident(ARGS_MAPPED),
                Expr::ident(ctx_name),
                some_value(Expr::Lit(ExprLit {
                    attrs: vec![],
                    lit: Lit::Str(LitStr::new(name.as_str(), Span::call_site())),
                })),
                some_value(Expr::Reference(ExprReference {
                    attrs: vec![],
                    and_token: Default::default(),
                    mutability: None,
                    expr: Box::new(self.arg_name_array(skip_table)),
                })),
            ]),
        });

        let init = LocalInit {
            eq_token: Default::default(),
            expr: Box::new(Expr::Try(ExprTry {
                attrs: vec![],
                expr: Box::new(init),
                question_token: Default::default(),
            })),
            diverge: None,
        };

        let mut names = Vec::with_capacity(self.inputs.len());
        let mut types = Vec::with_capacity(self.inputs.len());

        if skip_table {
            names.push(Pat::Wild(PatWild {
                attrs: vec![],
                underscore_token: Default::default(),
            }));
            types.push(Type::Path(TypePath::ident_segments(["mlua", "Table"])));
        }

        let mut user_data_idents = Vec::new();

        for (pat, ty) in self.args() {
            match ty {
                // references are assumed to be AnyUserData
                Type::Reference(type_ref) => {
                    names.push(pat.clone());
                    types.push(Type::Path(TypePath::ident_segments([
                        "mlua",
                        "AnyUserData",
                    ])));
                    user_data_idents.push((pat, type_ref));
                }
                other => {
                    names.push(pat);
                    types.push(other);
                }
            }
        }

        result.push(Stmt::Local(Local {
            attrs: vec![],
            let_token: Default::default(),
            pat: Pat::Type(PatType {
                attrs: vec![],
                pat: Box::new(Pat::Tuple(PatTuple {
                    attrs: vec![],
                    paren_token: Default::default(),
                    elems: Punctuated::from_iter(names),
                })),
                colon_token: Default::default(),
                ty: Box::new(Type::Tuple(TypeTuple {
                    paren_token: Default::default(),
                    elems: Punctuated::from_iter(types),
                })),
            }),
            init: Some(init),
            semi_token: Default::default(),
        }));

        for (pat, accessed) in user_data_idents {
            let is_mut = accessed.mutability.is_some();
            let ident;
            let ref_ident;
            let pat = if let Pat::Ident(ident_pat) = pat {
                ident = ident_pat.ident.clone();
                let ref_name = ident.to_string() + REF_SUFFIX;
                ref_ident = Ident::new(&ref_name, Span::call_site());

                Pat::Ident(PatIdent {
                    attrs: vec![],
                    by_ref: None,
                    mutability: if is_mut {
                        Some(Default::default())
                    } else {
                        None
                    },
                    ident: ref_ident.clone(),
                    subpat: None,
                })
            } else {
                return Err(Error::new_spanned(pat, "expected an identifier"));
            };

            let accessed = Type::Path(TypePath {
                qself: None,
                path: Path::ident_segments_generic(
                    ["std", "cell", if is_mut { "RefMut" } else { "Ref" }],
                    Some(GenericOptions {
                        leading_semi: false,
                        args: [GenericArgument::Type(accessed.elem.as_ref().to_owned())],
                    }),
                ),
            });

            let init = Some(LocalInit {
                eq_token: Default::default(),
                expr: Box::new(Expr::Try(ExprTry {
                    attrs: vec![],
                    expr: Box::new(Expr::MethodCall(ExprMethodCall {
                        attrs: vec![],
                        receiver: Box::new(Expr::Path(ExprPath {
                            attrs: vec![],
                            qself: None,
                            path: Path::from(ident.clone()),
                        })),
                        dot_token: Default::default(),
                        method: Ident::new("borrow", Span::call_site()),
                        turbofish: None,
                        paren_token: Default::default(),
                        args: Punctuated::new(),
                    })),
                    question_token: Default::default(),
                })),
                diverge: None,
            });

            result.push(Stmt::Local(Local {
                attrs: vec![],
                let_token: Default::default(),
                pat: Pat::Type(PatType {
                    attrs: vec![],
                    pat: Box::new(pat),
                    colon_token: Default::default(),
                    ty: Box::new(accessed),
                }),
                init,
                semi_token: Default::default(),
            }));

            result.push(Stmt::Local(Local {
                attrs: vec![],
                let_token: Default::default(),
                pat: Pat::Ident(PatIdent {
                    attrs: vec![],
                    by_ref: None,
                    mutability: None,
                    ident,
                    subpat: None,
                }),
                init: Some(LocalInit {
                    eq_token: Default::default(),
                    expr: Box::new(Expr::Reference(ExprReference {
                        attrs: vec![],
                        and_token: Default::default(),
                        mutability: if is_mut {
                            Some(Default::default())
                        } else {
                            None
                        },
                        expr: Box::new(Expr::Path(ExprPath {
                            attrs: vec![],
                            qself: None,
                            path: Path::from(ref_ident),
                        })),
                    })),
                    diverge: None,
                }),
                semi_token: Default::default(),
            }));
        }

        Ok(result)
    }

    fn arg_name_array(&self, skip_table: bool) -> Expr {
        let names = self
            .inputs
            .iter()
            .filter_map(|it| match it {
                FnArg::Typed(it) if matches!(it.pat.as_ref(), Pat::Ident(_)) => {
                    if let Pat::Ident(it) = it.pat.as_ref() {
                        Some(it)
                    } else {
                        unreachable!()
                    }
                }
                _ => None,
            })
            .map(|it| {
                Expr::Lit(ExprLit {
                    attrs: vec![],
                    lit: Lit::Str(LitStr::new(
                        it.ident.to_string().as_str(),
                        Span::call_site(),
                    )),
                })
            });

        Expr::Array(ExprArray {
            attrs: vec![],
            bracket_token: Default::default(),
            elems: if skip_table {
                Punctuated::from_iter(
                    std::iter::once(Expr::Lit(ExprLit {
                        attrs: vec![],
                        lit: Lit::Str(LitStr::new("self", Span::call_site())),
                    }))
                    .chain(names),
                )
            } else {
                Punctuated::from_iter(names)
            },
        })
    }

    fn args(&self) -> impl Iterator<Item = (Pat, Type)> + '_ {
        self.inputs.iter().filter_map(|it| match it {
            FnArg::Typed(t) if matches!(t.pat.as_ref(), Pat::Ident(_)) => {
                if let Pat::Ident(it) = t.pat.as_ref() {
                    Some((
                        Pat::Ident(PatIdent {
                            attrs: vec![],
                            by_ref: it.by_ref,
                            mutability: it.mutability,
                            ident: it.ident.clone(),
                            subpat: None,
                        }),
                        t.ty.as_ref().clone(),
                    ))
                } else {
                    unreachable!()
                }
            }
            _ => None,
        })
    }
}

struct LuaMethod {
    #[allow(unused)]
    source: ImplItemFn,
    signature: MethodSignature,
    ctx_lifetime: Option<Lifetime>,
    lua_block: Block,
}

impl LuaMethod {
    pub fn new(source: ImplItemFn) -> Result<Self> {
        let signature = MethodSignature::new(&source)?;

        let mut lua_block = source.block.clone();
        if let SignatureKind::Method { .. } = signature.kind {
            struct SelfMapper;
            impl VisitMut for SelfMapper {
                fn visit_ident_mut(&mut self, i: &mut Ident) {
                    if i == "self" {
                        *i = Ident::new(SELF_MAPPED, Span::call_site());
                    }
                }
            }
            SelfMapper.visit_block_mut(&mut lua_block);
        }

        let ctx_lifetime = signature.lua_ctx.clone().map(|it| it.0);

        if let Some(ctx_lifetime) = &ctx_lifetime {
            let mut found = false;
            for param in &source.sig.generics.params {
                if let GenericParam::Lifetime(LifetimeParam { lifetime, .. }) = param {
                    if lifetime.ident == ctx_lifetime.ident {
                        found = true;
                        break;
                    }
                }
            }

            if !found {
                return Err(Error::new_spanned(
                    ctx_lifetime,
                    "liftime not found in function generics",
                ));
            }
        }

        Ok(LuaMethod {
            source,
            ctx_lifetime,
            signature,
            lua_block,
        })
    }

    pub fn closure(&self, skip_table: bool) -> Result<ExprClosure> {
        let mut inputs = Punctuated::new();

        let ctx_name = if let Some((_, ctx)) = &self.signature.lua_ctx {
            inputs.push(Pat::Ident(PatIdent {
                attrs: vec![],
                by_ref: None,
                mutability: None,
                ident: ctx.clone(),
                subpat: None,
            }));
            ctx.to_string()
        } else {
            inputs.push(Pat::Ident(PatIdent {
                attrs: vec![],
                by_ref: None,
                mutability: None,
                ident: Ident::new(CTX_ERASED, Span::call_site()),
                subpat: None,
            }));
            CTX_ERASED.to_string()
        };

        if let SignatureKind::Method { .. } = self.signature.kind {
            inputs.push(Pat::Ident(PatIdent {
                attrs: vec![],
                by_ref: None,
                mutability: None,
                ident: Ident::new(SELF_MAPPED, Span::call_site()),
                subpat: None,
            }))
        }

        let insert_mapping = if self.signature.inputs.is_empty() {
            inputs.push(Pat::Tuple(PatTuple {
                attrs: vec![],
                paren_token: Default::default(),
                elems: Punctuated::new(),
            }));
            false
        } else {
            inputs.push(Pat::Ident(PatIdent {
                attrs: vec![],
                by_ref: None,
                mutability: None,
                ident: Ident::new(ARGS_MAPPED, Span::call_site()),
                subpat: None,
            }));
            true
        };

        let mut block = self.lua_block.clone();

        if insert_mapping {
            let mut modified = self
                .signature
                .block_setup_statements(&ctx_name, skip_table)?;
            modified.append(&mut block.stmts);
            block.stmts = modified;
        }

        let body = Box::new(Expr::Block(ExprBlock {
            attrs: vec![],
            label: None,
            block,
        }));

        Ok(ExprClosure {
            attrs: vec![],
            lifetimes: None,
            constness: None,
            movability: None,
            asyncness: self.signature.asyncness,
            capture: None,
            or1_token: Default::default(),
            inputs,
            or2_token: Default::default(),
            output: ReturnType::Default,
            body,
        })
    }
}

pub struct UserDataMetods {
    base: ItemImpl,
    generics: Generics,
    self_ty: Box<Type>,
    ctx_lifetime: Option<Lifetime>,
    methods: Vec<LuaMethod>,
    other: Vec<ImplItem>,
}

fn ctx_method(
    ctx_name: Ident,
    method: impl AsRef<str>,
    args: Punctuated<Expr, token::Comma>,
) -> Expr {
    Expr::MethodCall(ExprMethodCall {
        attrs: vec![],
        receiver: Box::new(Expr::Path(ExprPath {
            attrs: vec![],
            qself: None,
            path: Path::from(ctx_name),
        })),
        dot_token: Default::default(),
        method: Ident::new(method.as_ref(), Span::call_site()),
        turbofish: None,
        paren_token: Default::default(),
        args,
    })
}

fn globals(ctx_name: Ident) -> Expr {
    ctx_method(ctx_name, "globals", Punctuated::new())
}

impl UserDataMetods {
    fn method_register_calls(&self, recv: Expr) -> impl Iterator<Item = Result<Expr>> + '_ {
        self.methods.iter().map(move |m| {
            let sig = &m.signature;
            let name = sig.lua_name();

            let name = if sig.options.constructor {
                Expr::Lit(ExprLit {
                    attrs: vec![],
                    lit: Lit::Str(LitStr::new("__call", Span::call_site())),
                })
            } else {
                Expr::Lit(ExprLit {
                    attrs: vec![],
                    lit: Lit::Str(LitStr::new(name.as_str(), Span::call_site())),
                })
            };

            m.closure(false).map(|c| {
                Expr::MethodCall(ExprMethodCall {
                    attrs: vec![],
                    receiver: Box::new(recv.clone()),
                    dot_token: Default::default(),
                    method: sig.register_with(),
                    turbofish: None,
                    paren_token: Default::default(),
                    args: Punctuated::from_iter([name, Expr::Closure(c)]),
                })
            })
        })
    }

    pub fn base_impl(&self) -> ItemImpl {
        let mut result = self.base.clone();

        for item in &mut result.items {
            match item {
                ImplItem::Const(ImplItemConst { attrs, .. })
                | ImplItem::Type(ImplItemType { attrs, .. })
                | ImplItem::Macro(ImplItemMacro { attrs, .. }) => {
                    *attrs = attrs
                        .drain(..)
                        .filter(|it| !ItemOptions::check(&it.meta))
                        .collect::<Vec<_>>()
                }
                ImplItem::Fn(ImplItemFn { attrs, sig, .. }) => {
                    *attrs = attrs
                        .drain(..)
                        .filter(|it| !ItemOptions::check(&it.meta))
                        .collect::<Vec<_>>();

                    let out_type = match &sig.output {
                        ReturnType::Default => Type::Tuple(TypeTuple {
                            paren_token: Default::default(),
                            elems: Punctuated::new(),
                        }),
                        ReturnType::Type(_, it) => (**it).clone(),
                    };

                    sig.output = ReturnType::Type(
                        Default::default(),
                        Box::new(Type::Path(TypePath::ident_segments_generic(
                            ["mlua", "Result"],
                            Some(GenericOptions {
                                leading_semi: false,
                                args: [GenericArgument::Type(out_type)],
                            }),
                        ))),
                    );
                }
                _ => {}
            }
        }

        result
    }

    pub fn generate_userdata_impl(&self, _options: &AttributeOptions) -> Result<ItemImpl> {
        let method_registry = Ident::new("__lua_methods", Span::call_site());

        let block = Block {
            brace_token: Default::default(),
            stmts: self
                .method_register_calls(Expr::Path(ExprPath {
                    attrs: vec![],
                    qself: None,
                    path: Path::from(method_registry.clone()),
                }))
                .map(|it| it.map(|it| Stmt::Expr(it, Some(Default::default()))))
                .collect::<Result<Vec<_>>>()?,
        };

        let add_methods = parse_quote! {
            fn add_methods<'lua, M: mlua::UserDataMethods<'lua, Self>>(#method_registry: &mut M) #block
        };

        Ok(ItemImpl {
            attrs: vec![],
            defaultness: None,
            unsafety: None,
            impl_token: Default::default(),
            generics: self.generics.clone(),
            trait_: Some((
                None,
                Path::ident_segments(["mlua", "UserData"]),
                Default::default(),
            )),
            self_ty: self.self_ty.clone(),
            brace_token: Default::default(),
            items: vec![add_methods],
        })
    }

    pub fn generate_register_fn(&self, options: &AttributeOptions) -> Result<Option<ItemImpl>> {
        let lua_ctx = Ident::new("__lua_context", Span::call_site());

        let mut stmts = Vec::with_capacity(self.methods.len() + 3);

        let table_ident = Ident::new("__t_table", Span::call_site());
        stmts.push(Stmt::Local(Local {
            attrs: vec![],
            let_token: Default::default(),
            pat: Pat::Ident(PatIdent {
                attrs: vec![],
                by_ref: None,
                mutability: None,
                ident: table_ident.clone(),
                subpat: None,
            }),
            init: Some(LocalInit {
                eq_token: Default::default(),
                expr: Box::new(Expr::Try(ExprTry {
                    attrs: vec![],
                    expr: Box::new(ctx_method(
                        lua_ctx.clone(),
                        "create_table",
                        Punctuated::new(),
                    )),
                    question_token: Default::default(),
                })),
                diverge: None,
            }),
            semi_token: Default::default(),
        }));

        let statics = self
            .methods
            .iter()
            .filter(|it| matches!(it.signature.kind, SignatureKind::Function { .. }));

        let mut found_any = false;

        for m in statics {
            let sig = &m.signature;
            let c = m.closure(true)?;

            let function_reg = Expr::MethodCall(ExprMethodCall {
                attrs: vec![],
                receiver: Box::new(Expr::Path(ExprPath {
                    attrs: vec![],
                    qself: None,
                    path: Path::from(lua_ctx.clone()),
                })),
                dot_token: Default::default(),
                method: Ident::new("create_function", Span::call_site()),
                turbofish: None,
                paren_token: Default::default(),
                args: Punctuated::from_iter([Expr::Closure(c)]),
            });

            let function_reg = Expr::Try(ExprTry {
                attrs: vec![],
                expr: Box::new(function_reg),
                question_token: Default::default(),
            });

            let name = sig.lua_name();
            let name = if m.signature.options.constructor {
                Expr::Lit(ExprLit {
                    attrs: vec![],
                    lit: Lit::Str(LitStr::new("__call", Span::call_site())),
                })
            } else {
                Expr::Lit(ExprLit {
                    attrs: vec![],
                    lit: Lit::Str(LitStr::new(name.as_str(), Span::call_site())),
                })
            };

            let table_insert = Expr::MethodCall(ExprMethodCall {
                attrs: vec![],
                receiver: Box::new(Expr::Path(ExprPath {
                    attrs: vec![],
                    qself: None,
                    path: Path::from(table_ident.clone()),
                })),
                dot_token: Default::default(),
                method: Ident::new("set", Span::call_site()),
                turbofish: None,
                paren_token: Default::default(),
                args: Punctuated::from_iter([name, function_reg]),
            });

            let table_insert = Expr::Try(ExprTry {
                attrs: vec![],
                expr: Box::new(table_insert),
                question_token: Default::default(),
            });

            found_any = true;
            stmts.push(Stmt::Expr(table_insert, Some(Default::default())));
        }

        if !found_any {
            return Ok(None);
        }

        let set_metatable = Expr::MethodCall(ExprMethodCall {
            attrs: vec![],
            receiver: Box::new(Expr::Path(ExprPath {
                attrs: vec![],
                qself: None,
                path: Path::from(table_ident.clone()),
            })),
            dot_token: Default::default(),
            method: Ident::new("set_metatable", Span::call_site()),
            turbofish: None,
            paren_token: Default::default(),
            args: Punctuated::from_iter([some_value(cloned_value(Expr::Path(ExprPath {
                attrs: vec![],
                qself: None,
                path: Path::from(table_ident.clone()),
            })))]),
        });
        stmts.push(Stmt::Expr(set_metatable, Some(Default::default())));

        let base_name = options
            .lua_name
            .clone()
            .or_else(|| ty_base_name(&self.self_ty))
            .ok_or_else(|| {
                Error::new(
                    self.self_ty.span(),
                    "lua_methods attribute only works for named types",
                )
            })?;

        let set_table = Expr::MethodCall(ExprMethodCall {
            attrs: vec![],
            receiver: Box::new(globals(lua_ctx.clone())),
            dot_token: Default::default(),
            method: Ident::new("set", Span::call_site()),
            turbofish: None,
            paren_token: Default::default(),
            args: Punctuated::from_iter([
                Expr::Lit(ExprLit {
                    attrs: vec![],
                    lit: Lit::Str(LitStr::new(&base_name, Span::call_site())),
                }),
                Expr::Path(ExprPath {
                    attrs: vec![],
                    qself: None,
                    path: Path::from(table_ident),
                }),
            ]),
        });

        stmts.push(Stmt::Expr(set_table, None));

        let block = Block {
            brace_token: Default::default(),
            stmts,
        };

        let globals_fn = parse_quote! {
            fn register_globals<'lua>(#lua_ctx: &'lua mlua::Lua) -> Result<(), mlua::Error> #block
        };

        Ok(Some(ItemImpl {
            attrs: vec![],
            defaultness: None,
            unsafety: None,
            impl_token: Default::default(),
            generics: self.generics.clone(),
            trait_: None,
            self_ty: self.self_ty.clone(),
            brace_token: Default::default(),
            items: vec![globals_fn],
        }))
    }
}

impl Parse for UserDataMetods {
    fn parse(input: ParseStream) -> Result<Self> {
        let implementation = input.parse::<ItemImpl>()?;
        let base = implementation.clone();

        let mut result = UserDataMetods {
            base,
            generics: implementation.generics,
            self_ty: implementation.self_ty,
            ctx_lifetime: None,
            methods: Vec::with_capacity(implementation.items.len()),
            other: Vec::with_capacity(implementation.items.len()),
        };

        let mut errors = Vec::new();

        for item in implementation.items {
            if let ImplItem::Fn(func) = item {
                let method = match LuaMethod::new(func) {
                    Ok(it) => it,
                    Err(err) => {
                        errors.push(err);
                        continue;
                    }
                };

                if method.signature.options.skip {
                    continue;
                }

                match (&result.ctx_lifetime, &method.ctx_lifetime) {
                    (None, Some(assigned)) => result.ctx_lifetime = Some(assigned.clone()),
                    (Some(found), Some(tested)) if found.ident != tested.ident => {
                        return Err(Error::new_spanned(
                            tested,
                            "context lifetimes must be the same",
                        ));
                    }
                    _ => {}
                }
                result.methods.push(method);
            } else {
                result.other.push(item);
            }
        }

        if let Some(combined) = Error::from_many(errors) {
            Err(combined)
        } else {
            Ok(result)
        }
    }
}
