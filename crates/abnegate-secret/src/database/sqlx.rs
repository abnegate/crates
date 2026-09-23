use sqlx::Database;
use sqlx::Decode;
use sqlx::Encode;
use sqlx::Type;
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;

use crate::value::SecretValue;

impl<Driver: Database> Type<Driver> for SecretValue
where
    String: Type<Driver>,
{
    fn type_info() -> Driver::TypeInfo {
        <String as Type<Driver>>::type_info()
    }

    fn compatible(info: &Driver::TypeInfo) -> bool {
        <String as Type<Driver>>::compatible(info)
    }
}

impl<'r, Driver: Database> Decode<'r, Driver> for SecretValue
where
    String: Decode<'r, Driver>,
{
    fn decode(value: Driver::ValueRef<'r>) -> Result<Self, BoxDynError> {
        <String as Decode<'r, Driver>>::decode(value).map(SecretValue::new)
    }
}

impl<'q, Driver: Database> Encode<'q, Driver> for SecretValue
where
    String: Encode<'q, Driver>,
{
    fn encode_by_ref(&self, buffer: &mut Driver::ArgumentBuffer) -> Result<IsNull, BoxDynError> {
        <String as Encode<'q, Driver>>::encode_by_ref(self.expose_buffer(), buffer)
    }
}

#[cfg(test)]
mod tests {
    use sqlx::Any;
    use sqlx::Arguments;
    use sqlx::any::AnyArguments;

    use super::*;

    fn stores_as_text<Driver>()
    where
        Driver: Database,
        SecretValue: Type<Driver> + for<'q> Encode<'q, Driver> + for<'r> Decode<'r, Driver>,
    {
    }

    #[test]
    fn is_a_column_wherever_text_is() {
        stores_as_text::<Any>();
        assert_eq!(
            <SecretValue as Type<Any>>::type_info(),
            <String as Type<Any>>::type_info()
        );
    }

    #[test]
    fn binds_as_a_query_argument() {
        let mut arguments = AnyArguments::default();
        arguments.add(SecretValue::new("sk-live-plain")).unwrap();
        assert_eq!(arguments.len(), 1);
    }
}
