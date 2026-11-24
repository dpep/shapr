#[used]
#[link_section = ".init_array"]
static B: extern "C" fn() = b;
#[used]
#[link_section = ".init_array"]
static A: extern "C" fn() = a;
extern "C" fn a() {}
extern "C" fn b() {}
