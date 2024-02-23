use lua_methods::UserDataMetods;
use options::AttributeOptions;
use quote::ToTokens;
use syn::parse_macro_input;

mod lua_methods;
mod options;
mod util;

#[proc_macro_attribute]
pub fn lua_methods(
    options: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let options = parse_macro_input!(options as AttributeOptions);

    let model: UserDataMetods = parse_macro_input!(input as UserDataMetods);
    let mut result = match model.generate_userdata_impl(&options) {
        Ok(it) => it.into_token_stream(),
        Err(err) => return err.to_compile_error().into_token_stream().into(),
    };

    let register_fn = match model.generate_register_fn(&options) {
        Ok(it) => it,
        Err(err) => return err.to_compile_error().into_token_stream().into(),
    };

    if let Some(register_fn) = register_fn {
        result.extend(register_fn.into_token_stream());
    }

    result.into()
}
