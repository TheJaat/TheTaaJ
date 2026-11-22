// cxxlib.cpp - checks of C++ on TheTaaJ: the language's runtime
// (exceptions, typeinfo and dynamic_cast, constructors before main and
// destructors after it, function-local statics, new and delete) and the
// standard library (strings, containers, algorithms, iostreams, string
// streams, <charconv>, <format>, <regex>, <chrono>, smart pointers,
// <optional>, <variant>, <any>, <functional>).
//
//   cxxlib          every check; "c++: N checks, all passed", and last of
//                   all, from a destructor after main has returned,
//                   "c++: the objects outside main were destroyed"
//   cxxlib throw    an exception nobody catches: std::terminate, which
//                   says what it was, and exit code 134 (abort)

#include <algorithm>
#include <any>
#include <cassert>
#include <charconv>
#include <chrono>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <deque>
#include <format>
#include <functional>
#include <iomanip>
#include <iostream>
#include <list>
#include <map>
#include <memory>
#include <numeric>
#include <optional>
#include <random>
#include <regex>
#include <set>
#include <sstream>
#include <stdexcept>
#include <string>
#include <string_view>
#include <typeinfo>
#include <unordered_map>
#include <variant>
#include <vector>

static int checks;
static int failures;

static void check(bool ok, const char *what)
{
    checks++;
    if (!ok) {
        failures++;
        std::printf("cxxlib: FAILED: %s\n", what);
    }
}

// ---- before main, and after it ----

// Made before main by the constructors in .init_array; their order
// within one file is the order they are written in.
static std::vector<std::string> made;

struct Recorder {
    const char *name;
    explicit Recorder(const char *n) : name(n) { made.push_back(n); }
};

static Recorder first_one("first");
static Recorder second_one("second");

// Destroyed after main returns, by a handler __cxa_atexit registered when
// it was made. Its message is the program's very last line.
struct LastWords {
    ~LastWords() { std::printf("c++: the objects outside main were destroyed\n"); }
};
static LastWords last_words;

// A C function marked constructor runs before main too.
static int constructor_ran;
__attribute__((constructor)) static void c_style_constructor() { constructor_ran = 1; }

// ---- exceptions ----

static int alive; // objects of Counted not yet destroyed

struct Counted {
    Counted() { alive++; }
    Counted(const Counted &) { alive++; }
    ~Counted() { alive--; }
};

struct Failure : std::runtime_error {
    int code;
    Failure(const std::string &what, int c) : std::runtime_error(what), code(c) {}
};

// Three frames deep, each with an object to be destroyed on the way out.
static int deep(int levels)
{
    Counted here;
    if (levels == 0)
        throw Failure("from the bottom", 42);
    return deep(levels - 1) + 1;
}

struct Half {
    Counted a;
    Counted b;
    Half() : a(), b() { throw std::logic_error("half made"); }
};

static char *volatile escaped;

static void exceptions()
{
    try {
        deep(3);
        check(false, "an exception was not thrown");
    } catch (const Failure &f) {
        check(f.code == 42 && std::string(f.what()) == "from the bottom", "the exception's contents");
        check(alive == 0, "every frame's object destroyed on the way up");
    }

    // Caught as a base class, rethrown, caught again.
    int caught = 0;
    try {
        try {
            throw Failure("again", 7);
        } catch (const std::exception &) {
            caught++;
            throw;
        }
    } catch (const Failure &f) {
        caught += f.code;
    }
    check(caught == 8, "throw; rethrows the same exception");

    // Any type can be thrown, and catch (...) takes them all.
    try {
        throw 3.5;
    } catch (int) {
        check(false, "a double caught as an int");
    } catch (double d) {
        check(d == 3.5, "a double thrown and caught");
    }
    try {
        throw std::string("text");
    } catch (...) {
        checks++;
    }

    // A constructor that throws: the members already made are destroyed.
    try {
        Half h;
    } catch (const std::logic_error &) {
        check(alive == 0, "the members of a half-made object destroyed");
    }

    // The library throws too.
    std::vector<int> v(3);
    try {
        (void)v.at(10);
        check(false, "vector::at did not throw");
    } catch (const std::out_of_range &) {
        checks++;
    }
    try {
        (void)std::stoi("not a number");
        check(false, "stoi did not throw");
    } catch (const std::invalid_argument &) {
        checks++;
    }

    // An exception kept, and thrown again elsewhere.
    std::exception_ptr kept;
    try {
        throw Failure("kept", 9);
    } catch (...) {
        kept = std::current_exception();
    }
    try {
        std::rethrow_exception(kept);
    } catch (const Failure &f) {
        check(f.code == 9, "std::exception_ptr carries the exception");
    }

    // new that cannot be satisfied throws std::bad_alloc. (The pointer
    // is kept where the compiler cannot see it used or not: a new whose
    // memory goes nowhere may be left out altogether, and then nothing
    // is asked for.)
    try {
        escaped = new char[std::size_t(1) << 60];
        delete[] escaped;
        check(false, "a huge new did not throw");
    } catch (const std::bad_alloc &) {
        checks++;
    }
}

// ---- types at run time ----

struct Shape {
    virtual ~Shape() = default;
    virtual double area() const = 0;
};
struct Square : Shape {
    double side;
    explicit Square(double s) : side(s) {}
    double area() const override { return side * side; }
};
struct Circle : Shape {
    double radius;
    explicit Circle(double r) : radius(r) {}
    double area() const override { return 3.0 * radius * radius; }
};
struct Named {
    virtual ~Named() = default;
    virtual const char *name() const { return "named"; }
};
struct NamedSquare : Square, Named {
    NamedSquare() : Square(2) {}
    const char *name() const override { return "named square"; }
};

static void types()
{
    std::vector<std::unique_ptr<Shape>> shapes;
    shapes.push_back(std::make_unique<Square>(3));
    shapes.push_back(std::make_unique<Circle>(2));
    double total = 0;
    for (const auto &s : shapes)
        total += s->area();
    check(total == 21, "virtual functions");

    Shape *s = shapes[0].get();
    check(dynamic_cast<Square *>(s) != nullptr, "dynamic_cast down to the right type");
    check(dynamic_cast<Circle *>(s) == nullptr, "dynamic_cast down to the wrong type");
    check(typeid(*s) == typeid(Square), "typeid of an object through its base");
    check(std::strcmp(typeid(int).name(), "i") == 0, "typeid's name");

    // Across: from one base of NamedSquare to its other base.
    NamedSquare ns;
    Shape *as_shape = &ns;
    Named *as_named = dynamic_cast<Named *>(as_shape);
    check(as_named != nullptr && std::strcmp(as_named->name(), "named square") == 0, "dynamic_cast across");

    try {
        Circle &c = dynamic_cast<Circle &>(*s);
        (void)c;
        check(false, "dynamic_cast to a wrong reference did not throw");
    } catch (const std::bad_cast &) {
        checks++;
    }
}

// ---- memory ----

struct alignas(64) Wide {
    char bytes[64];
};

static int counter()
{
    // Made the first time through, and only then (a guard variable).
    static int calls = 0;
    static std::string made_once = std::to_string(++calls);
    return std::stoi(made_once);
}

static void memory()
{
    auto *w = new Wide;
    check(reinterpret_cast<std::uintptr_t>(w) % 64 == 0, "new of an over-aligned type is aligned");
    delete w;
    std::vector<Wide> many(10);
    check(reinterpret_cast<std::uintptr_t>(many.data()) % 64 == 0, "a vector of them is aligned");

    // An over-aligned block must be given back whole: its start is
    // `alignment` bytes before what the program was handed. If delete
    // gave back less, nothing would crash - the rest would only be lost
    // - but the same request again would no longer fit where the first
    // one was. (So this depends on the heap reusing a block it was just
    // given back for the next request that fits it; TheTaaJ's does.)
    void *first = ::operator new(65536, std::align_val_t{4096});
    ::operator delete(first, std::align_val_t{4096});
    void *second = ::operator new(65536, std::align_val_t{4096});
    check(second == first, "delete of an over-aligned block gives all of it back");
    ::operator delete(second, std::align_val_t{4096});

    auto shared = std::make_shared<std::string>("shared");
    std::weak_ptr<std::string> weak = shared;
    {
        auto another = shared;
        check(shared.use_count() == 2, "shared_ptr counts its owners");
    }
    shared.reset();
    check(weak.expired(), "weak_ptr knows when the object is gone");

    counter();
    counter();
    check(counter() == 1, "a function's static is made once");
}

// ---- the library ----

static void strings()
{
    std::string s = "Hello";
    s += ", ";
    s.append("TheTaaJ");
    check(s == "Hello, TheTaaJ" && s.size() == 14, "string building");
    check(s.find("TaaJ") == 10 && s.substr(7, 3) == "The", "find and substr");
    std::string big(1000, 'x');
    check(big.size() == 1000 && big[999] == 'x', "a string on the heap");
    check(std::to_string(-123) == "-123" && std::to_string(2.5) == "2.500000", "to_string");
    check(std::stoi("  42") == 42 && std::stod("6.25") == 6.25, "stoi and stod");
    std::string_view view = "a view";
    check(view.substr(2) == "view", "string_view");

    char digits[32];
    auto [end, error] = std::to_chars(digits, digits + sizeof digits, 0.1);
    check(error == std::errc() && std::string(digits, end) == "0.1", "to_chars: the shortest that reads back");
    // (libc++ 18 reads only integers with from_chars.)
    const char *number = "-1234567";
    long back = 0;
    std::from_chars(number, number + std::strlen(number), back);
    check(back == -1234567, "from_chars");

    check(std::format("{} + {} = {:>4}", 2, 3, 5) == "2 + 3 =    5", "std::format");
    check(std::format("{:.3f} {:x} {:08b}", 3.14159, 255, 5) == "3.142 ff 00000101", "std::format's numbers");
}

static void containers()
{
    std::vector<int> v{5, 3, 9, 1, 7};
    std::sort(v.begin(), v.end());
    check((v == std::vector<int>{1, 3, 5, 7, 9}), "sort");
    check(std::binary_search(v.begin(), v.end(), 7), "binary_search");
    check(std::accumulate(v.begin(), v.end(), 0) == 25, "accumulate");

    std::map<std::string, int> m;
    m["two"] = 2;
    m["one"] = 1;
    m["three"] = 3;
    check(m.begin()->first == "one" && m.size() == 3, "map keeps its keys in order");

    std::unordered_map<int, std::string> u;
    for (int i = 0; i < 1000; i++)
        u[i] = std::to_string(i * i);
    check(u.size() == 1000 && u[31] == "961", "unordered_map of a thousand");

    std::set<int> set{3, 1, 3, 2, 1};
    check(set.size() == 3, "set drops repeats");
    std::list<int> list{1, 2, 3};
    list.push_front(0);
    list.reverse();
    check(list.front() == 3 && list.back() == 0, "list");
    std::deque<int> d;
    for (int i = 0; i < 100; i++)
        d.push_front(i);
    check(d.front() == 99 && d[99] == 0, "deque");

    // A big sort: random numbers, the same ones every run.
    std::mt19937 random(12345);
    std::vector<unsigned> many(100000);
    for (auto &x : many)
        x = random();
    std::sort(many.begin(), many.end());
    check(std::is_sorted(many.begin(), many.end()), "a hundred thousand sorted");
    std::mt19937 again(5489);
    check(again() == 3499211612u, "mt19937's first number is the standard's");
}

static void streams()
{
    std::ostringstream out;
    out << "x=" << 42 << " y=" << std::fixed << std::setprecision(2) << 3.14159 << " " << std::hex << 255;
    check(out.str() == "x=42 y=3.14 ff", "ostringstream with manipulators");

    std::istringstream in("12 apples 3.5");
    int n = 0;
    std::string word;
    double weight = 0;
    in >> n >> word >> weight;
    check(n == 12 && word == "apples" && weight == 3.5, "istringstream reads numbers and words");

    std::istringstream lines("one\ntwo\nthree");
    std::string line;
    int count = 0;
    while (std::getline(lines, line))
        count++;
    check(count == 3, "getline");
}

static void the_rest()
{
    std::optional<int> maybe;
    check(!maybe && maybe.value_or(5) == 5, "optional, empty");
    maybe = 3;
    check(maybe && *maybe == 3, "optional, full");

    std::variant<int, std::string> v = std::string("text");
    check(std::holds_alternative<std::string>(v), "variant");
    std::any a = 1.5;
    check(std::any_cast<double>(a) == 1.5, "any");

    int base = 10;
    std::function<int(int)> add = [base](int x) { return base + x; };
    check(add(5) == 15, "std::function holding a lambda");

    std::regex date(R"((\d{4})-(\d{2})-(\d{2}))");
    std::smatch match;
    std::string text = "on 2026-11-22, in the evening";
    check(std::regex_search(text, match, date) && match[2] == "11", "regex");

    auto start = std::chrono::steady_clock::now();
    auto later = std::chrono::steady_clock::now();
    check(later >= start, "steady_clock does not go back");

    check(std::sqrt(2.0) == 1.4142135623730951 && std::floor(-2.5) == -3 && std::fmod(7.5, 2) == 1.5,
          "<cmath>'s exact functions");
}

int main(int argc, char **argv)
{
    if (argc > 1 && std::strcmp(argv[1], "throw") == 0) {
        // Nobody catches this: std::terminate, then abort.
        throw std::runtime_error("nobody catches this");
    }
    check(made.size() == 2 && made[0] == "first" && made[1] == "second",
          "objects outside main made before it, in order");
    check(constructor_ran == 1, "a C constructor ran before main");
    exceptions();
    types();
    memory();
    strings();
    containers();
    streams();
    the_rest();
    std::cout << "c++: " << checks << " checks, " << (failures ? "some FAILED" : "all passed") << std::endl;
    return failures ? 1 : 0;
}
