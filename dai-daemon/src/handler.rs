use {
    serde_json::{Map, Value},
    std::{
        env, fs,
        path::{Component, Path, PathBuf},
        pin::Pin,
    },
};

pub type Call<T> = Result<T, String>;

pub type Func = Box<dyn Fn(&Value) -> Pin<Box<dyn Future<Output = Call<Value>> + '_>>>;

pub fn make<H, T, const N: usize>(handler: H, names: &'static [&'static str; N]) -> Func
where
    H: Handler<T> + Clone + 'static,
{
    const { assert!(H::N == N, "each argument must be named") };

    Box::new(move |value| {
        let handler = handler.clone();
        Box::pin(async {
            handler.call(Arguments { args: value, names }).await?;
            Ok(Value::Object(Map::new()))
        })
    })
}

pub struct Arguments<'val> {
    args: &'val Value,
    names: &'static [&'static str],
}

pub trait Handler<T> {
    const N: usize;
    async fn call(self, args: Arguments<'_>) -> Call<()>;
}

impl<F> Handler<fn()> for F
where
    F: AsyncFnOnce() -> Call<()>,
{
    const N: usize = 0;

    async fn call(self, _: Arguments<'_>) -> Call<()> {
        (self)().await?;
        Ok(())
    }
}

impl<A, F> Handler<fn(A)> for F
where
    A: FromValue + ?Sized,
    F: AsyncFnOnce(A::Me<'_>) -> Call<()>,
{
    const N: usize = 1;

    async fn call(self, args: Arguments<'_>) -> Call<()> {
        let a = <A>::from_value(args.args, args.names[0])?;
        (self)(a).await?;
        Ok(())
    }
}

impl<A, B, F> Handler<fn(A, B)> for F
where
    A: FromValue + ?Sized,
    B: FromValue + ?Sized,
    F: AsyncFnOnce(A::Me<'_>, B::Me<'_>) -> Call<()>,
{
    const N: usize = 2;

    async fn call(self, args: Arguments<'_>) -> Call<()> {
        let a = <A>::from_value(args.args, args.names[0])?;
        let b = <B>::from_value(args.args, args.names[1])?;
        (self)(a, b).await?;
        Ok(())
    }
}

trait FromValue {
    type Me<'val>;
    fn from_value<'val>(value: &'val Value, name: &str) -> Call<Self::Me<'val>>;
}

impl FromValue for str {
    type Me<'val> = &'val Self;

    fn from_value<'val>(value: &'val Value, name: &str) -> Call<Self::Me<'val>> {
        value[name]
            .as_str()
            .ok_or_else(|| format!("key {name} not found"))
    }
}

impl FromValue for String {
    type Me<'val> = Self;

    fn from_value<'val>(value: &'val Value, name: &str) -> Call<Self::Me<'val>> {
        <str>::from_value(value, name).map(Self::from)
    }
}

impl FromValue for i64 {
    type Me<'val> = Self;

    fn from_value<'val>(value: &'val Value, name: &str) -> Call<Self::Me<'val>> {
        value[name]
            .as_i64()
            .ok_or_else(|| format!("key {name} not found"))
    }
}

impl FromValue for u64 {
    type Me<'val> = Self;

    fn from_value<'val>(value: &'val Value, name: &str) -> Call<Self::Me<'val>> {
        value[name]
            .as_u64()
            .ok_or_else(|| format!("key {name} not found"))
    }
}

impl FromValue for PathBuf {
    type Me<'val> = Self;

    fn from_value<'val>(value: &'val Value, name: &str) -> Call<Self::Me<'val>> {
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

        Ok(hostdir)
    }
}
