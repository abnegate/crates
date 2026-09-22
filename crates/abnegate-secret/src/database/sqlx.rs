use sqlx::{Database, Decode, Encode, Type, encode::IsNull, error::BoxDynError};
use zeroize::Zeroize;

use crate::value::SecretValue;

impl<DB: Database> Type<DB> for SecretValue
where
    String: Type<DB>,
{
    fn type_info() -> DB::TypeInfo {
        <String as Type<DB>>::type_info()
    }

    fn compatible(info: &DB::TypeInfo) -> bool {
        <String as Type<DB>>::compatible(info)
    }
}

impl<'r, DB: Database> Decode<'r, DB> for SecretValue
where
    String: Decode<'r, DB>,
{
    fn decode(value: DB::ValueRef<'r>) -> Result<Self, BoxDynError> {
        <String as Decode<'r, DB>>::decode(value).map(SecretValue::new)
    }
}

impl<'q, DB: Database> Encode<'q, DB> for SecretValue
where
    String: Encode<'q, DB>,
{
    fn encode_by_ref(&self, buffer: &mut DB::ArgumentBuffer) -> Result<IsNull, BoxDynError> {
        let mut exposed = self.expose().to_string();
        let encoded = <String as Encode<'q, DB>>::encode_by_ref(&exposed, buffer);
        exposed.zeroize();
        encoded
    }
}
