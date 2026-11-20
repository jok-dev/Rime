//! Pins the type-annotation span against a full_moon quirk (2.x and 3.0):
//! TypeInfo::Tuple's range()/tokens() miss the closing parenthesis, so the
//! lowering rebalances against the source. The span must select exactly the
//! written type - no leading `: `, closing bracket included.
#[test]
fn tuple_return_type_span_includes_closing_paren() {
    let src = "local function f(x: number): (CFrame, CFrame)\n\treturn a, b\nend\n";
    let m = rime_syntax::parse(src).unwrap();
    let body = m
        .bodies
        .iter()
        .find(|b| b.return_ty.is_some())
        .expect("function body");
    let ty = body.return_ty.as_ref().unwrap();
    assert_eq!(&src[ty.span.clone()], "(CFrame, CFrame)");
    let param_ty = body.params[0].ty.as_ref().unwrap();
    assert_eq!(&src[param_ty.span.clone()], "number");
}
