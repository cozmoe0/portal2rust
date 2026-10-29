//! Language semantics: scripts run against expected printed output.

use squirrel::Vm;

fn capture(host: &mut dyn std::any::Any, text: &str) {
    if let Some(out) = host.downcast_mut::<String>() {
        out.push_str(text);
    }
}

fn run(src: &str) -> String {
    let mut vm = Vm::new();
    vm.print = capture;
    let mut out = String::new();
    let root = vm.root_value();
    if let Err(e) = vm.run_script(&mut out, src.as_bytes(), "test", &root) {
        out.push_str(&format!("<error: {}>", e.message()));
    }
    out
}

macro_rules! check {
    ($name:ident, $src:expr, $expected:expr) => {
        #[test]
        fn $name() {
            assert_eq!(run($src), $expected);
        }
    };
}

check!(
    arithmetic,
    r#"
    print(7 / 2); print(" "); print(7 % 3); print(" "); print(7.0 / 2); print(" ");
    print(1 + 2 * 3); print(" "); print(-5 / 2); print(" "); print(2147483647 + 1); print(" ");
    print(1 << 4); print(" "); print(-16 >> 2); print(" "); print(-16 >>> 28); print(" "); print(5 & 3 | 8 ^ 1)
"#,
    "3 1 3.5 7 -2 -2147483648 16 -4 15 9"
);

check!(
    float_strings,
    r#"
    print(0.1 + 0.2); print(" "); print(1.0); print(" "); print(1.0e6 * 1.0); print(" "); print(1.5e-5); print(" ");
    print("a" + 1 + 2.5 + null + true)
"#,
    "0.3 1 1e+006 1.5e-005 a12.5nulltrue"
);

check!(
    division_errors,
    r#"
    try { local x = 1 / 0 } catch (e) { print(e) }
    print(" ");
    try { local y = 1 % 0 } catch (e) { print(e) }
"#,
    "division by zero modulo by zero"
);

check!(
    comparisons,
    r#"
    print(1 == 1.0); print(" "); print("a" < "b"); print(" "); print(null < 1); print(" ");
    print(2 > 1.5); print(" "); print({} == {}); print(" "); print("abc" == "ab" + "c")
"#,
    "true true true true false true"
);

check!(
    truthiness,
    r#"
    foreach (v in [null, 0, 0.0, false, "", [], 1]) print(v ? "T" : "F")
"#,
    "FFFFTTT"
);

check!(
    closures_capture_by_value,
    r#"
    local x = 1
    local f = function() { return x }
    x = 2
    print(f()); print(" ")
    local function counter() { local n = 0; return function() { n++; return n } }
    local c = counter(); c(); print(c())
"#,
    "1 2"
);

check!(
    tables_and_root_fallback,
    r#"
    g <- 10
    local t = { a = 1, b = 2 }
    t.c <- 3
    print(t.a + t.b + t.c); print(" ")
    try { t.d = 4 } catch (e) { print(e) } print(" ")
    function f() { return g }
    print(f()); print(" ")
    local scope = {}
    scope.setdelegate(getroottable())
    local h = compilestring("g = 11; return g")
    print(h.call(scope)); print(" "); print(g)
"#,
    "6 the index 'd' does not exist 10 11 11"
);

check!(
    env_this_for_plain_calls,
    r#"
    function who() { return this.name }
    local obj = { name = "obj", call_who = function() { return who() } }
    print(obj.call_who())
"#,
    "obj"
);

check!(
    delegates_and_metamethods,
    r#"
    local base = { greet = function() { return "hi " + name } }
    local t = delegate base : { name = "t" }
    print(t.greet()); print(" ")
    local mt = { _get = function(k) { return "missing " + k } }
    local u = {}.setdelegate(mt)
    print(u.foo); print(" "); print(t.getdelegate() == base); print(" "); print(t.parent == base)
"#,
    "hi t missing foo true true"
);

check!(
    parent_keyword,
    r#"
    local p = { v = 5 }
    local c = { f = function() { return parent.v } }.setdelegate(p)
    print(c.f())
"#,
    "5"
);

check!(
    classes,
    r#"
    class Animal {
        name = null
        static kingdom = "animalia"
        constructor(n) { name = n }
        function speak() { return name + " makes a sound" }
    }
    class Dog extends Animal {
        function speak() { return name + " barks" }
    }
    local d = Dog("rex")
    print(d.speak()); print(" "); print(d instanceof Animal); print(" "); print(Dog.kingdom); print(" ")
    print(typeof d); print(" "); print(typeof Dog)
    try { d.foo = 1 } catch (e) { print(" " + e) }
"#,
    "rex barks true animalia instance class the index 'foo' does not exist"
);

check!(
    class_metamethods,
    r#"
    class V {
        x = 0
        constructor(a) { x = a }
        function _add(o) { return V(x + o.x) }
        function _tostring() { return "V(" + x + ")" }
        function _cmp(o) { return x - o.x }
        function _typeof() { return "Vector" }
    }
    local a = V(1), b = V(2)
    print(a + b); print(" "); print(a < b); print(" "); print(typeof a)
"#,
    "V(3) true Vector"
);

check!(
    generators,
    r#"
    function gen(n) { for (local i = 0; i < n; i++) yield i * 10 }
    local g = gen(3)
    print(resume g); print(" "); print(resume g); print(" ")
    foreach (v in gen(4)) print(v + ",")
    print(" " + g.getstatus())
"#,
    "0 10 0,10,20,30, suspended"
);

check!(
    exceptions,
    r#"
    function thrower() { throw { code = 42 } }
    try { thrower() } catch (e) { print(e.code) } print(" ")
    try { local a = []; a[5] } catch (e) { print(e) } print(" ")
    try { null() } catch (e) { print(e) }
"#,
    "42 the index '5' does not exist attempt to call 'null'"
);

check!(
    loops_break_continue,
    r#"
    local s = ""
    for (local i = 0; i < 10; i++) {
        if (i == 2) continue
        if (i == 5) break
        try { if (i == 4) continue; s += i } catch (e) {}
    }
    local j = 0
    while (true) { j++; if (j > 3) break }
    do { j++ } while (j < 6)
    print(s + " " + j)
"#,
    "013 6"
);

check!(
    switch_fallthrough,
    r#"
    function f(x) {
        local r = ""
        switch (x) {
            case 1: r += "one "
            case 2: r += "two "; break
            case "s": r += "str "; break
            default: r += "other "
        }
        return r
    }
    print(f(1) + f(2) + f("s") + f(9))
"#,
    "one two two str other "
);

check!(
    foreach_kinds,
    r#"
    foreach (i, v in ["a", "b"]) print(i + v)
    foreach (i, c in "hi") print(" " + i + ":" + c)
    local t = {}
    for (local i = 0; i < 8; i++) t[i] <- i * i
    local keys = ""
    foreach (k, v in t) keys += k
    print(" " + keys)
"#,
    "0a1b 0:104 1:105 01234567"
);

check!(
    defaults_and_varargs,
    r#"
    function f(a, b = 2, c = 3) { return a + b + c }
    print(f(1) + " " + f(1, 1) + " " + f(1, 1, 1))
    function g(a, ...) { local s = a; for (local i = 0; i < vargc; i++) s += vargv[i]; return s }
    print(" " + g(1, 2, 3))
    try { f() } catch (e) { print(" " + e) }
"#,
    "6 5 3 6 wrong number of parameters"
);

check!(
    consts_and_enums,
    r#"
    const LIMIT = 5
    enum Color { Red, Green = 10, Blue }
    print(LIMIT + Color.Blue)
"#,
    "16"
);

check!(
    strings,
    r#"
    local s = "Hello World"
    print(s.len() + " " + s.slice(0, 5) + " " + s.slice(-5) + " " + s.find("World") + " " + s.tolower())
    print(" " + format("%d-%5.2f-%s-%x", 42, 3.14159, "x", 255))
    print(" " + "12".tointeger() * 2 + " " + "1.5".tofloat() + " " + (65).tochar())
    local parts = split("a,b,,c", ",")
    print(" " + parts.len() + " " + strip("  pad  ") + "|")
"#,
    "11 Hello World 6 hello world 42- 3.14-x-ff 24 1.5 A 3 pad|"
);

check!(
    arrays,
    r#"
    local a = [5, 3, 8, 1]
    a.sort()
    local s = ""
    foreach (v in a) s += v
    a.push(9); a.insert(0, 0); a.remove(1)
    local b = a.slice(1, 3)
    a.reverse()
    print(s + " " + a.len() + " " + a[0] + " " + b.len() + " " + a.top())
    local c = [3, 1, 2]
    c.sort(function(x, y) { return y - x })
    print(" " + c[0] + c[1] + c[2])
"#,
    "1358 5 9 2 0 321"
);

check!(
    typeof_clone_delete_in,
    r#"
    local t = { a = 1 }
    local u = clone t
    u.a = 2
    print(t.a + " " + u.a + " " + ("a" in t) + " " + ("b" in t))
    delete t.a
    print(" " + ("a" in t) + " " + typeof 1 + " " + typeof 1.0 + " " + typeof "" + " " + typeof null + " " + typeof print)
"#,
    "1 2 true false false integer float string null native function"
);

check!(
    bindenv,
    r#"
    local env = { v = "bound" }
    function getv() { return v }
    local b = getv.bindenv(env)
    print(b())
"#,
    "bound"
);

check!(
    regexp,
    r#"
    local re = regexp("^On.*Output$")
    print(re.match("OnTurretOutput") + " " + re.match("Nope"))
"#,
    "true false"
);

check!(
    weakrefs,
    r#"
    local t = {}
    local w = t.weakref()
    print(w.ref() == t)
"#,
    "true"
);

check!(compile_errors_report_lines, "local x = \n\n)", "<error: expression expected, found ')'>");
