use {
    serde_json::{Map, Value},
    std::{
        convert::Infallible,
        env, fs,
        path::{Component, Path, PathBuf},
    },
};

pub type Call<T> = Result<T, String>;

pub fn args<'val, A>(args: &'val Value, names: &'static [&'static str]) -> Arguments<'val, A>
where
    A: Default,
{
    let a = A::default();
    Arguments { args, names, a }
}

pub struct Arguments<'val, A> {
    args: &'val Value,
    names: &'static [&'static str],
    a: A,
}

impl<'val, A> Arguments<'val, A> {
    pub async fn call<H, T>(&'val mut self, handler: H) -> Call<Value>
    where
        H: Handler<'val, T, Alloc = A>,
    {
        handler.call(self).await?;
        Ok(Value::Object(Map::new()))
    }
}

pub trait Handler<'val, T> {
    type Alloc: Default + 'static;
    async fn call(self, args: &'val mut Arguments<'val, Self::Alloc>) -> Call<()>;
}

impl<'val, A, F> Handler<'val, (A,)> for F
where
    A: FromValue<'val>,
    F: AsyncFnOnce(A) -> Call<()>,
{
    type Alloc = (Alloc<A::Alloc>,);

    async fn call(self, args: &'val mut Arguments<'val, Self::Alloc>) -> Call<()> {
        let a = <A>::from_value(args.args, args.names[0], &mut args.a.0)?;
        (self)(a).await?;
        Ok(())
    }
}

impl<'val, A, B, F> Handler<'val, (A, B)> for F
where
    A: FromValue<'val>,
    B: FromValue<'val>,
    F: AsyncFnOnce(A, B) -> Call<()>,
{
    type Alloc = (Alloc<A::Alloc>, Alloc<B::Alloc>);

    async fn call(self, args: &'val mut Arguments<'val, Self::Alloc>) -> Call<()> {
        let a = <A>::from_value(args.args, args.names[0], &mut args.a.0)?;
        let b = <B>::from_value(args.args, args.names[1], &mut args.a.1)?;
        (self)(a, b).await?;
        Ok(())
    }
}

impl<'val, A, B, C, F> Handler<'val, (A, B, C)> for F
where
    A: FromValue<'val>,
    B: FromValue<'val>,
    C: FromValue<'val>,
    F: AsyncFnOnce(A, B, C) -> Call<()>,
{
    type Alloc = (Alloc<A::Alloc>, Alloc<B::Alloc>, Alloc<C::Alloc>);

    async fn call(self, args: &'val mut Arguments<'val, Self::Alloc>) -> Call<()> {
        let a = <A>::from_value(args.args, args.names[0], &mut args.a.0)?;
        let b = <B>::from_value(args.args, args.names[1], &mut args.a.1)?;
        let c = <C>::from_value(args.args, args.names[2], &mut args.a.2)?;
        (self)(a, b, c).await?;
        Ok(())
    }
}

pub struct Alloc<T>(Option<T>);

impl<T> Alloc<T> {
    fn alloc(&mut self, value: T) -> &mut T {
        self.0.insert(value)
    }

    fn into_inner(self) -> T {
        self.0.expect("allocated value")
    }
}

impl<T> Default for Alloc<T> {
    fn default() -> Self {
        Self(None)
    }
}

pub trait FromValue<'val>: Sized {
    type Alloc: 'static;
    fn from_value(value: &'val Value, name: &str, a: &'val mut Alloc<Self::Alloc>) -> Call<Self>;
}

impl<'val, R> FromValue<'val> for R
where
    R: FromBorrowedValue<'val>,
{
    type Alloc = Infallible;

    fn from_value(value: &'val Value, name: &str, _: &'val mut Alloc<Self::Alloc>) -> Call<Self> {
        R::from_borrowed_value(value, name)
    }
}

trait FromBorrowedValue<'val>: Sized {
    fn from_borrowed_value(value: &'val Value, name: &str) -> Call<Self>;
}

impl<'val> FromBorrowedValue<'val> for &'val str {
    fn from_borrowed_value(value: &'val Value, name: &str) -> Call<Self> {
        value[name]
            .as_str()
            .ok_or_else(|| format!("key {name} not found"))
    }
}

impl FromBorrowedValue<'_> for String {
    fn from_borrowed_value(value: &Value, name: &str) -> Call<Self> {
        <&str>::from_borrowed_value(value, name).map(Self::from)
    }
}

impl FromBorrowedValue<'_> for u64 {
    fn from_borrowed_value(value: &Value, name: &str) -> Call<Self> {
        value[name]
            .as_u64()
            .ok_or_else(|| format!("key {name} not found"))
    }
}

impl FromBorrowedValue<'_> for i64 {
    fn from_borrowed_value(value: &Value, name: &str) -> Call<Self> {
        value[name]
            .as_i64()
            .ok_or_else(|| format!("key {name} not found"))
    }
}

impl<'val> FromValue<'val> for &'val Path {
    type Alloc = PathBuf;

    fn from_value(value: &'val Value, name: &str, a: &'val mut Alloc<Self::Alloc>) -> Call<Self> {
        let path = Path::new(
            value[name]
                .as_str()
                .ok_or_else(|| format!("key {name} not found"))?,
        );

        if path.is_absolute() {
            return Err(format!("path is absolute: {}", path.display()));
        }

        for component in path.components() {
            let contains = match component {
                Component::Prefix(_) => Some("prefix"),
                Component::RootDir => Some("root"),
                Component::CurDir => Some("curdir"),
                Component::ParentDir => Some("parentdir"),
                Component::Normal(_) => None,
            };

            if let Some(component) = contains {
                return Err(format!("path contains: {component}"));
            }
        }

        let hostdir = env::current_dir()
            .map_err(|e| format!("failed to read current dir: {e}"))?
            .join(path);

        if let Some(parent) = hostdir.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                format!("failed to create directories at {}: {e}", parent.display())
            })?;
        }

        Ok(a.alloc(hostdir))
    }
}

impl FromBorrowedValue<'_> for PathBuf {
    fn from_borrowed_value(value: &Value, name: &str) -> Call<Self> {
        let mut a = Alloc::default();
        <&Path>::from_value(value, name, &mut a)?;
        Ok(a.into_inner())
    }
}
