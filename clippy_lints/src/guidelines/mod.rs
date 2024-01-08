mod blocking_op_in_async;
mod extern_without_repr;
mod fallible_memory_allocation;
mod invalid_char_range;
mod passing_string_to_c_functions;
mod ptr;
mod return_stack_address;
mod unconstrained_numeric_literal;
mod unsafe_block_in_proc_macro;
mod untrusted_lib_loading;

use clippy_utils::diagnostics::span_lint_and_help;
use clippy_utils::{def_path_def_ids, fn_def_id};
use rustc_data_structures::fx::FxHashSet;
use rustc_hir as hir;
use rustc_hir::def_id::{DefIdSet, LocalDefId};
use rustc_hir::hir_id::HirIdSet;
use rustc_hir::intravisit;
use rustc_lint::{LateContext, LateLintPass};
use rustc_session::{declare_tool_lint, impl_lint_pass};
use rustc_span::Span;

declare_clippy_lint! {
    /// ### What it does
    /// Checks for direct usage of external functions that modify memory
    /// without concerning about memory safety, such as `memcpy`, `strcpy`, `strcat` etc.
    ///
    /// ### Why is this bad?
    /// These function can be dangerous when used incorrectly,
    /// which could potentially introduce vulnerablities such as buffer overflow to the software.
    ///
    /// ### Example
    /// ```rust,ignore
    /// extern "C" {
    ///     fn memcpy(dest: *mut c_void, src: *const c_void, n: size_t) -> *mut c_void;
    /// }
    /// let ptr = unsafe { memcpy(dest, src, size); }
    /// // Or use via libc
    /// let ptr = unsafe { libc::memcpy(dest, src, size); }
    /// ```
    #[clippy::version = "1.70.0"]
    pub MEM_UNSAFE_FUNCTIONS,
    nursery,
    "use of potentially dangerous external functions"
}

declare_clippy_lint! {
    /// ### What it does
    /// Checks for dynamic library loading from untrusted sources,
    /// such as the data read from some IO functions.
    ///
    /// Loader functions and IO functions are configurable.
    ///
    /// ### Why is this bad?
    /// Loading dynamic libs from untrusted sources could make the software more vulnerable,
    /// as the attackers might be able to modify the source to load any plugins they desire,
    /// causing arbitrary code execution.
    ///
    /// ### Example
    /// ```rust,ignore
    /// let mut buf = String::new();
    /// f.read_to_string(&mut buf).unwrap();
    ///
    /// unsafe {
    ///     let _a = libloading::Library::new(&buf);
    /// }
    /// ```
    #[clippy::version = "1.70.0"]
    pub UNTRUSTED_LIB_LOADING,
    nursery,
    "attempt to load dynamic library from untrusted source"
}

declare_clippy_lint! {
    /// ### What it does
    /// Detects when passing Rust native strings (`&str` and `String`) to FFI functions.
    ///
    /// ### Why is this bad?
    /// String might be represented differently between Rust and other languages (espacially C).
    /// For example, in Rust, string pointers are wide pointers, which includes start pointer and the length.
    /// Whereas in C, string pointers are narrow pointers, which does not have length info, instead it enforces
    /// every C strings must end with `\0`. Thus, passing a Rust string's pointer to exteral C functions
    /// might not guarenteed to work.
    ///
    /// ### Example
    /// ```rust,ignore
    /// let s: String = String::from("hello world");
    /// unsafe {
    ///     some_extern_fn(s.as_ptr() as *const _);
    /// }
    /// ```
    /// Use `CString` or `CStr` instead:
    /// ```rust,ignore
    /// let s: CString = CString::new("hello world")?;
    /// unsafe {
    ///     some_extern_fn(s.as_ptr());
    /// }
    /// ```
    #[clippy::version = "1.70.0"]
    pub PASSING_STRING_TO_C_FUNCTIONS,
    nursery,
    "passing string or str to extern C function"
}

declare_clippy_lint! {
    /// ### What it does
    /// Checks for manually memory allocation without validating its input and output.
    ///
    /// ### Why is this bad?
    /// Such allocation might fail, causing unexpected software behavior.
    ///
    /// When using external C api such as `malloc`, a failed allocation call returns null pointer.
    /// Which might leads to null pointer dereferencing error if the pointer location was later accessed.
    ///
    /// ### Example
    /// ```rust,ignore
    /// unsafe fn alloc_mem(size: usize) {
    ///     let p = malloc(size);
    ///     // deref `p` somewhere
    /// }
    /// ```
    /// Use instead:
    /// ```rust,ignore
    /// unsafe fn alloc_mem(size: usize) {
    ///     assert!(size <= MAX_ALLOWED_SIZE);
    ///     let p = malloc(size);
    ///     assert!(!p.is_null())
    ///     // deref `p` somewhere
    /// }
    /// ```
    #[clippy::version = "1.70.0"]
    pub FALLIBLE_MEMORY_ALLOCATION,
    nursery,
    "memory allocation without checking arguments and result"
}

declare_clippy_lint! {
    /// ### What it does
    /// Checks for calling certain function that could block its thread in an async context.
    ///
    /// ### Why is this bad?
    /// Blocking a thread prevents tasks being swapped, causing other tasks to stop running
    /// until the thread is no longer blocked, which might lead to unexpected behavior.
    ///
    /// ### Example
    /// ```rust
    /// use std::time::Duration;
    /// pub async fn foo() {
    ///     std::thread::sleep(Duration::from_secs(5));
    /// }
    /// ```
    /// Use instead:
    /// ```rust,ignore
    /// use std::time::Duration;
    /// pub async fn foo() {
    ///     tokio::time::sleep(Duration::from_secs(5));
    /// }
    /// ```
    #[clippy::version = "1.70.0"]
    pub BLOCKING_OP_IN_ASYNC,
    nursery,
    "calling blocking funtions in an async context"
}

declare_clippy_lint! {
    /// ### What it does
    /// Checks for unsafe block written in procedural macro
    ///
    /// ### Why is this bad?
    /// It hides the unsafe code, making the safety of expended code unsound.
    ///
    /// ### Known problems
    /// Possible FP when the user uses proc-macro to generate a function with unsafe block in it.
    ///
    /// ### Example
    /// ```rust,ignore
    /// #[proc_macro]
    /// pub fn rprintf(input: TokenStream) -> TokenStream {
    ///     let expr = parse_macro_input!(input as syn::Expr);
    ///     quote!({
    ///         unsafe {
    ///             // unsafe operation
    ///         }
    ///     })
    /// }
    ///
    /// // This allows users to use this macro without `unsafe` block
    /// rprintf!();
    /// ```
    /// Use instead:
    /// ```rust,ignore
    /// #[proc_macro]
    /// pub fn rprintf(input: TokenStream) -> TokenStream {
    ///     let expr = parse_macro_input!(input as syn::Expr);
    ///     quote!({
    ///         // unsafe operation
    ///     })
    /// }
    ///
    /// // When using this macro, an outer `unsafe` block is needed,
    /// // making the safety of this macro much clearer.
    /// unsafe { rprintf!(); }
    /// ```
    #[clippy::version = "1.70.0"]
    pub UNSAFE_BLOCK_IN_PROC_MACRO,
    nursery,
    "using unsafe block in procedural macro's definition"
}

declare_clippy_lint! {
    /// ### What it does
    ///
    /// ### Why is this bad?
    ///
    /// ### Example
    /// ```rust,ignore
    /// struct Foo3 {
    ///     a: libc::c_char,
    ///     b: libc::c_int,
    ///     c: libc::c_longlong,
    /// }
    /// extern "C" fn c_abi_fn4(arg_one: u32, arg_two: *const Foo3) {}
    /// ```
    /// Use instead:
    /// ```rust,ignore
    /// #[repr(C)]
    /// struct Foo3 {
    ///     a: libc::c_char,
    ///     b: libc::c_int,
    ///     c: libc::c_longlong,
    /// }
    /// extern "C" fn c_abi_fn4(arg_one: u32, arg_two: *const Foo3) {}
    /// ```
    #[clippy::version = "1.72.0"]
    pub EXTERN_WITHOUT_REPR,
    pedantic,
    "Should use repr to specifing data layout when struct is used in FFI"
}

declare_clippy_lint! {
    /// ### What it does
    /// Checks for non-reentrant functions.
    ///
    /// ### Why is this bad?
    /// This makes code safer, especially in the context of concurrency.
    ///
    /// ### Example
    /// ```rust,ignore
    /// let _tm = libc::localtime(&0i64 as *const libc::time_t);
    /// ```
    /// Use instead:
    /// ```rust,ignore
    /// let res = libc::malloc(std::mem::size_of::<libc::tm>());
    ///
    /// libc::locatime_r(&0i64 as *const libc::time_t, res);
    /// ```
    #[clippy::version = "1.70.0"]
    pub NON_REENTRANT_FUNCTIONS,
    nursery,
    "this function is a non-reentrant-function"
}

declare_clippy_lint! {
    /// ### What it does
    /// Checks for raw pointers that are initialized or assigned as null pointers,
    /// but immediately dereferenced without any pre-caution.
    ///
    /// ### Why is this bad?
    /// Dereferencing null pointer is an undefined behavior.
    ///
    /// ### Known problems
    /// This lint only checks direct reference of null pointer, which means if the null pointer
    /// was referenced somewhere before de dereference, this lint would skip it entirely.
    /// For example, if a null pointer was passed to
    /// a function, but that function still does not assign value to its address, then it
    /// would be assumed non-null even though it wasn't.
    ///
    /// ### Example
    /// ```rust,ignore
    /// let a: *mut i8 = std::ptr::null_mut();
    /// let _ = unsafe { *a };
    /// ```
    ///
    /// Use instead:
    /// ```rust,ignore
    /// let a: *mut i8 = std::ptr::null_mut();
    /// unsafe { *a = 10_i8; }
    /// let _ = unsafe { *a };
    /// ```
    #[clippy::version = "1.68.0"]
    pub NULL_PTR_DEREFERENCE,
    nursery,
    "dereferencing null pointers"
}

declare_clippy_lint! {
    /// ### What it does
    /// Checks for freeing a pointer after which already got freed.
    ///
    /// ### Why is this bad?
    /// Pointer double free is a common weakness in terms of memory security,
    /// it leads program crash or give access to attackers.
    ///
    /// ### Example
    /// ```rust,ignore
    /// let ptr: *const u8 = std::ptr::null();
    /// unsafe {
    ///     free(ptr);
    ///     free(ptr);
    /// }
    /// ```
    /// Use instead:
    /// ```rust,ignore
    /// let mut ptr: *const u8 = std::ptr::null();
    /// unsafe {
    ///     free(ptr);
    ///     ptr = std::ptr::null();
    ///
    ///     if !ptr.is_null() {
    ///         free(ptr);
    ///     }
    /// }
    /// ```
    #[clippy::version = "1.68.0"]
    pub PTR_DOUBLE_FREE,
    nursery,
    "pointer double free"
}

declare_clippy_lint! {
    /// ### What it does
    /// Detects pointer dereferencing after it got freed/deallocated.
    ///
    /// ### Why is this bad?
    /// After freeing a pointer, the address it previously points to might no longer
    /// being held by the program. But the pointer is still valid, reading or writing
    /// the pointer is a undefined behavior.
    ///
    /// ### Example
    /// ```rust,ignore
    /// unsafe {
    ///     free(ptr);
    ///     let val = *ptr;
    /// }
    /// ```
    #[clippy::version = "1.68.0"]
    pub DANGLING_PTR_DEREFERENCE,
    nursery,
    "dereferencing dangling pointers"
}

declare_clippy_lint! {
    /// ### What it does
    /// Detects block returning a memory address to a locally defined variable that stores in stack.
    ///
    /// ### Why is this bad?
    /// Accessing such memory address is guaranteed to be undefined behavior.
    ///
    /// ### Example
    /// ```rust
    /// fn foo() -> *const i32 {
    ///     let val: i32 = 100;
    ///     &val as *const _
    /// }
    /// ```
    #[clippy::version = "1.68.0"]
    pub RETURN_STACK_ADDRESS,
    nursery,
    "returning pointer that points to stack address"
}

// Experimental lint, may be removed in the future
declare_clippy_lint! {
    /// ### What it does
    /// Checks for converting to `char`` from a out-of-range unsigned int.
    ///
    /// ### Why is this bad?
    /// Conversion from an unsigned integer to a char can only be valid in a
    /// certain range. User should be warned on any out-of-range convertion attempts.
    ///
    /// ### Example
    /// ```rust
    /// let x = char::from_u32(0xDE01);
    /// ```
    #[clippy::version = "1.74.0"]
    pub INVALID_CHAR_RANGE,
    nursery,
    "converting to char from a out-of-range unsigned int"
}

declare_clippy_lint! {
    /// ### What it does
    /// Checks for usage of unconstrained numeric literals in variable initialization.
    ///
    /// This lint is differ from `default_numeric_fallback` in the following perspectives:
    /// 1. It only checks numeric literals in a local binding.
    /// 2. It lints all kinds of numeric literals rather than `i32` and `f64`.
    ///
    /// ### Why is this bad?
    /// Initializing a numeric type without labeling its type could cause default numeric fallback.
    ///
    /// ### Example
    /// ```rust
    /// let i = 10;
    /// let f = 1.23;
    /// ```
    ///
    /// Use instead:
    /// ```rust
    /// let i = 10i32;
    /// let f = 1.23f64;
    /// ```
    #[clippy::version = "1.74.0"]
    pub UNCONSTRAINED_NUMERIC_LITERAL,
    nursery,
    "usage of unconstrained numeric literals in variable initialization"
}

/// Helper struct with user configured path-like functions, such as `std::fs::read`,
/// and a set for `def_id`s which should be filled during checks.
///
/// NB: They might not have a one-on-one relation.
#[derive(Clone, Default, Debug)]
pub struct FnPathsAndIds {
    pub paths: Vec<String>,
    pub ids: DefIdSet,
}

impl FnPathsAndIds {
    fn with_paths(paths: Vec<String>) -> Self {
        Self {
            paths,
            ..Default::default()
        }
    }
}

#[derive(Clone, Default)]
pub struct LintGroup {
    mem_uns_fns: FnPathsAndIds,
    mem_alloc_fns: FnPathsAndIds,
    io_fns: FnPathsAndIds,
    lib_loading_fns: FnPathsAndIds,
    blocking_fns: FnPathsAndIds,
    non_reentrant_fns: FnPathsAndIds,
    allow_io_blocking_ops: bool,
    macro_call_sites: FxHashSet<Span>,
    alloc_size_check_fns: Vec<String>,
    mem_free_fns: FnPathsAndIds,
    /// (For [`return_stack_address`]) Stores block's hir_id after visit them,
    /// so the the same block can be skipped in the next iteration of `check_block`.
    visited_blocks: HirIdSet,
}

impl LintGroup {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mem_uns_fns: Vec<String>,
        io_fns: Vec<String>,
        lib_loading_fns: Vec<String>,
        allow_io_blocking_ops: bool,
        alloc_size_check_fns: Vec<String>,
        mem_alloc_fns: Vec<String>,
        non_reentrant_fns: Vec<String>,
        mem_free_fns: Vec<String>,
    ) -> Self {
        Self {
            mem_uns_fns: FnPathsAndIds::with_paths(mem_uns_fns),
            mem_alloc_fns: FnPathsAndIds::with_paths(mem_alloc_fns),
            io_fns: FnPathsAndIds::with_paths(io_fns),
            lib_loading_fns: FnPathsAndIds::with_paths(lib_loading_fns),
            non_reentrant_fns: FnPathsAndIds::with_paths(non_reentrant_fns),
            allow_io_blocking_ops,
            alloc_size_check_fns,
            mem_free_fns: FnPathsAndIds::with_paths(mem_free_fns),
            ..Default::default()
        }
    }
}

impl_lint_pass!(LintGroup => [
    MEM_UNSAFE_FUNCTIONS,
    UNTRUSTED_LIB_LOADING,
    PASSING_STRING_TO_C_FUNCTIONS,
    FALLIBLE_MEMORY_ALLOCATION,
    BLOCKING_OP_IN_ASYNC,
    UNSAFE_BLOCK_IN_PROC_MACRO,
    EXTERN_WITHOUT_REPR,
    NON_REENTRANT_FUNCTIONS,
    NULL_PTR_DEREFERENCE,
    PTR_DOUBLE_FREE,
    DANGLING_PTR_DEREFERENCE,
    RETURN_STACK_ADDRESS,
    INVALID_CHAR_RANGE,
    UNCONSTRAINED_NUMERIC_LITERAL,
]);

impl<'tcx> LateLintPass<'tcx> for LintGroup {
    fn check_fn(
        &mut self,
        cx: &LateContext<'tcx>,
        kind: intravisit::FnKind<'tcx>,
        _decl: &'tcx hir::FnDecl<'_>,
        body: &'tcx hir::Body<'_>,
        span: Span,
        _def_id: LocalDefId,
    ) {
        if !matches!(kind, intravisit::FnKind::Closure) {
            blocking_op_in_async::check_fn(cx, kind, body, span, &self.blocking_fns.ids);
        }
    }

    fn check_crate(&mut self, cx: &LateContext<'tcx>) {
        add_configured_fn_ids(cx, &mut self.mem_uns_fns);
        add_configured_fn_ids(cx, &mut self.mem_alloc_fns);
        add_configured_fn_ids(cx, &mut self.io_fns);
        add_configured_fn_ids(cx, &mut self.lib_loading_fns);
        add_configured_fn_ids(cx, &mut self.non_reentrant_fns);
        add_configured_fn_ids(cx, &mut self.mem_free_fns);

        blocking_op_in_async::init_blacklist_ids(cx, self.allow_io_blocking_ops, &mut self.blocking_fns.ids);
    }

    fn check_item(&mut self, cx: &LateContext<'tcx>, item: &'tcx hir::Item<'_>) {
        if let hir::ItemKind::ForeignMod { items, .. } = item.kind {
            add_extern_fn_ids(items, &mut self.mem_uns_fns);
            add_extern_fn_ids(items, &mut self.mem_alloc_fns);
            add_extern_fn_ids(items, &mut self.io_fns);
            add_extern_fn_ids(items, &mut self.lib_loading_fns);
            add_extern_fn_ids(items, &mut self.non_reentrant_fns);
            add_extern_fn_ids(items, &mut self.mem_free_fns);
        }
        extern_without_repr::check_item(cx, item);
    }

    fn check_expr(&mut self, cx: &LateContext<'tcx>, expr: &'tcx hir::Expr<'_>) {
        if let hir::ExprKind::Call(_func, params) = &expr.kind {
            if let Some(fn_did) = fn_def_id(cx, expr) {
                if self.non_reentrant_fns.ids.contains(&fn_did) {
                    lint_non_reentrant_fns(cx, expr);
                }
                if self.mem_uns_fns.ids.contains(&fn_did) {
                    lint_mem_unsafe_fns(cx, expr);
                }
                if self.lib_loading_fns.ids.contains(&fn_did) {
                    untrusted_lib_loading::check_expr(cx, expr, params, &self.io_fns.ids);
                }
                if self.mem_alloc_fns.ids.contains(&fn_did) {
                    fallible_memory_allocation::check_expr(cx, expr, params, fn_did, &self.alloc_size_check_fns);
                }
                if self.mem_free_fns.ids.contains(&fn_did) {
                    ptr::check_call(cx, expr, &self.mem_free_fns.ids);
                }
                passing_string_to_c_functions::check_expr(cx, expr, fn_did, params);
                invalid_char_range::check_call(cx, expr, params, fn_did);
            }
        } else {
            blocking_op_in_async::check_expr(cx, expr, &self.blocking_fns.ids);
            unsafe_block_in_proc_macro::check(cx, expr, &mut self.macro_call_sites);
            ptr::check_assign(cx, expr);
        }
    }

    fn check_local(&mut self, cx: &LateContext<'tcx>, local: &'tcx hir::Local<'tcx>) {
        ptr::check_local(cx, local);
        unconstrained_numeric_literal::check_local(cx, local);
    }

    fn check_block(&mut self, cx: &LateContext<'tcx>, block: &'tcx hir::Block<'tcx>) {
        return_stack_address::check(cx, block, &mut self.visited_blocks);
    }
}

/// Resolve and insert the `def_id` of user configure functions if:
///
/// 1. They are the full path like string, such as: `krate::module::func`.
/// 2. They are function names in libc crate.
fn add_configured_fn_ids(cx: &LateContext<'_>, fns: &mut FnPathsAndIds) {
    for fn_name in &fns.paths {
        // Path like function names such as `libc::foo` or `aa::bb::cc::bar`,
        // this only works with dependencies.
        if fn_name.contains("::") {
            let path: Vec<&str> = fn_name.split("::").collect();
            for did in def_path_def_ids(cx, path.as_slice()) {
                fns.ids.insert(did);
            }
        }
        // Plain function names, then we should take its libc variant into account
        else {
            for did in def_path_def_ids(cx, &["libc", fn_name]) {
                fns.ids.insert(did);
            }
        }
    }
}

/// Resolve and insert the `def_id` of functions declared in an `extern` block
fn add_extern_fn_ids(items: &[hir::ForeignItemRef], fns: &mut FnPathsAndIds) {
    for f_item in items {
        if fns.paths.contains(&f_item.ident.as_str().to_string()) {
            let f_did = f_item.id.hir_id().owner.def_id.to_def_id();
            fns.ids.insert(f_did);
        }
    }
}

/// Peels all casts and return the inner most (non-cast) expression.
///
/// i.e.
///
/// ```ignore
/// some_expr as *mut i8 as *mut i16 as *mut i32 as *mut i64
/// ```
///
/// Will return expression for `some_expr`.
pub fn peel_casts<'a, 'tcx>(maybe_cast_expr: &'a hir::Expr<'tcx>) -> &'a hir::Expr<'tcx> {
    if let hir::ExprKind::Cast(expr, _) = maybe_cast_expr.kind {
        peel_casts(expr)
    } else {
        maybe_cast_expr
    }
}

// These lint logics are simple enough that don't need their own file.
fn lint_non_reentrant_fns(cx: &LateContext<'_>, expr: &hir::Expr<'_>) {
    span_lint_and_help(
        cx,
        NON_REENTRANT_FUNCTIONS,
        expr.span,
        "use of non-reentrant function",
        None,
        "consider using its reentrant counterpart",
    );
}
fn lint_mem_unsafe_fns(cx: &LateContext<'_>, expr: &hir::Expr<'_>) {
    span_lint_and_help(
        cx,
        MEM_UNSAFE_FUNCTIONS,
        expr.span,
        "use of potentially dangerous memory manipulation function",
        None,
        "consider using its safe version",
    );
}
