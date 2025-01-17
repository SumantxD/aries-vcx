mod ed25519;

use std::{collections::HashMap, str};

use async_std::sync::RwLock;
use hex::FromHex;
use indy_api_types::errors::prelude::*;

use indy_utils::crypto::{
    base64, chacha20poly1305_ietf, chacha20poly1305_ietf::gen_nonce_and_encrypt_detached,
    ed25519_box, ed25519_sign,
};

use crate::utils::crypto::base58::{FromBase58, ToBase58};

use crate::{
    domain::crypto::{
        did::{Did, DidValue, MyDidInfo, TheirDid, TheirDidInfo},
        key::{Key, KeyInfo},
    },
    utils::crypto::verkey_builder::{build_full_verkey, split_verkey, verkey_get_cryptoname},
};

use ed25519::ED25519CryptoType;

const DEFAULT_CRYPTO_TYPE: &str = "ed25519";

//TODO fix this crypto trait so it matches the functions below
//TODO create a second crypto trait for additional functions
trait CryptoType: Send + Sync {
    fn crypto_box(
        &self,
        sk: &ed25519_sign::SecretKey,
        vk: &ed25519_sign::PublicKey,
        doc: &[u8],
        nonce: &ed25519_box::Nonce,
    ) -> IndyResult<Vec<u8>>;

    fn crypto_box_open(
        &self,
        sk: &ed25519_sign::SecretKey,
        vk: &ed25519_sign::PublicKey,
        doc: &[u8],
        nonce: &ed25519_box::Nonce,
    ) -> IndyResult<Vec<u8>>;

    fn gen_nonce(&self) -> ed25519_box::Nonce;

    fn create_key(
        &self,
        seed: Option<&ed25519_sign::Seed>,
    ) -> IndyResult<(ed25519_sign::PublicKey, ed25519_sign::SecretKey)>;

    fn validate_key(&self, _vk: &ed25519_sign::PublicKey) -> IndyResult<()>;

    fn sign(&self, sk: &ed25519_sign::SecretKey, doc: &[u8])
        -> IndyResult<ed25519_sign::Signature>;

    fn verify(
        &self,
        vk: &ed25519_sign::PublicKey,
        doc: &[u8],
        signature: &ed25519_sign::Signature,
    ) -> IndyResult<bool>;

    fn crypto_box_seal(&self, vk: &ed25519_sign::PublicKey, doc: &[u8]) -> IndyResult<Vec<u8>>;

    fn crypto_box_seal_open(
        &self,
        vk: &ed25519_sign::PublicKey,
        sk: &ed25519_sign::SecretKey,
        doc: &[u8],
    ) -> IndyResult<Vec<u8>>;
}

pub struct CryptoService {
    crypto_types: RwLock<HashMap<&'static str, Box<dyn CryptoType>>>,
}

impl CryptoService {
    pub(crate) fn new() -> CryptoService {
        let crypto_types = {
            let mut types = HashMap::<&'static str, Box<dyn CryptoType>>::new();
            types.insert(DEFAULT_CRYPTO_TYPE, Box::new(ED25519CryptoType::new()));
            RwLock::new(types)
        };

        CryptoService { crypto_types }
    }

    pub(crate) fn defualt_crypto_type() -> &'static str {
        DEFAULT_CRYPTO_TYPE
    }

    pub(crate) async fn create_key(&self, key_info: &KeyInfo) -> IndyResult<Key> {
        trace!("create_key > key_info {:?}", secret!(key_info));

        let crypto_type_name = key_info
            .crypto_type
            .as_ref()
            .map(String::as_str)
            .unwrap_or(DEFAULT_CRYPTO_TYPE);

        let crypto_types = self.crypto_types.read().await;

        let crypto_type = crypto_types.get(crypto_type_name).ok_or_else(|| {
            err_msg(
                IndyErrorKind::UnknownCrypto,
                format!("KeyInfo contains unknown crypto: {}", crypto_type_name),
            )
        })?;

        let seed = self.convert_seed(key_info.seed.as_ref().map(String::as_ref))?;
        let (vk, sk) = crypto_type.create_key(seed.as_ref())?;
        let mut vk = vk[..].to_base58();
        let sk = sk[..].to_base58();

        if !crypto_type_name.eq(DEFAULT_CRYPTO_TYPE) {
            // Use suffix with crypto type name to store crypto type inside of vk
            vk = format!("{}:{}", vk, crypto_type_name);
        }

        let key = Key::new(vk, sk);

        let res = Ok(key);
        trace!("create_key < {:?}", res);
        res
    }

    pub(crate) async fn create_my_did(&self, my_did_info: &MyDidInfo) -> IndyResult<(Did, Key)> {
        trace!("create_my_did > my_did_info {:?}", secret!(my_did_info));

        let crypto_type_name = my_did_info
            .crypto_type
            .as_ref()
            .map(String::as_str)
            .unwrap_or(DEFAULT_CRYPTO_TYPE);

        let crypto_types = self.crypto_types.read().await;

        let crypto_type = crypto_types.get(crypto_type_name).ok_or_else(|| {
            err_msg(
                IndyErrorKind::UnknownCrypto,
                format!("MyDidInfo contains unknown crypto: {}", crypto_type_name),
            )
        })?;

        let seed = self.convert_seed(my_did_info.seed.as_ref().map(String::as_ref))?;
        let (vk, sk) = crypto_type.create_key(seed.as_ref())?;
        let did = match my_did_info.did {
            Some(ref did) => did.clone(),
            _ if my_did_info.cid == Some(true) => DidValue::new(
                &vk[..].to_vec().to_base58(),
                my_did_info.ledger_type.as_deref(),
                my_did_info
                    .method_name
                    .as_ref()
                    .map(|method| method.0.as_str()),
            )?,
            _ => DidValue::new(
                &vk[0..16].to_vec().to_base58(),
                my_did_info.ledger_type.as_deref(),
                my_did_info
                    .method_name
                    .as_ref()
                    .map(|method| method.0.as_str()),
            )?,
        };

        let mut vk = vk[..].to_base58();
        let sk = sk[..].to_base58();

        if !crypto_type_name.eq(DEFAULT_CRYPTO_TYPE) {
            // Use suffix with crypto type name to store crypto type inside of vk
            vk = format!("{}:{}", vk, crypto_type_name);
        }

        let did = (Did::new(did, vk.clone()), Key::new(vk, sk));

        let res = Ok(did);
        trace!("create_my_did < {:?}", res);
        res
    }

    pub(crate) async fn create_their_did(
        &self,
        their_did_info: &TheirDidInfo,
    ) -> IndyResult<TheirDid> {
        trace!("create_their_did > their_did_info {:?}", their_did_info);

        // Check did is correct Base58
        let _ = self.validate_did(&their_did_info.did)?;

        let verkey = build_full_verkey(
            &their_did_info.did.to_unqualified().0,
            their_did_info.verkey.as_ref().map(String::as_str),
        )?;

        self.validate_key(&verkey).await?;

        let did = TheirDid {
            did: their_did_info.did.clone(),
            verkey,
        };

        let res = Ok(did);
        trace!("create_their_did < {:?}", res);
        res
    }

    pub(crate) async fn sign(&self, my_key: &Key, doc: &[u8]) -> IndyResult<Vec<u8>> {
        trace!("sign > my_key {:?} doc {:?}", my_key, doc);

        let crypto_type_name = verkey_get_cryptoname(&my_key.verkey);
        let crypto_types = self.crypto_types.read().await;

        let crypto_type = crypto_types.get(crypto_type_name).ok_or_else(|| {
            err_msg(
                IndyErrorKind::UnknownCrypto,
                format!(
                    "Trying to sign message with unknown crypto: {}",
                    crypto_type_name
                ),
            )
        })?;

        let my_sk = ed25519_sign::SecretKey::from_slice(
            &my_key.signkey.as_str().from_base58()?.as_slice(),
        )?;

        let signature = crypto_type.sign(&my_sk, doc)?[..].to_vec();

        let res = Ok(signature);
        trace!("sign < {:?}", res);
        res
    }

    pub(crate) async fn verify(
        &self,
        their_vk: &str,
        msg: &[u8],
        signature: &[u8],
    ) -> IndyResult<bool> {
        trace!(
            "verify > their_vk {:?} msg {:?} signature {:?}",
            their_vk,
            msg,
            signature
        );

        let (their_vk, crypto_type_name) = split_verkey(their_vk);

        let crypto_types = self.crypto_types.read().await;

        let crypto_type = crypto_types.get(crypto_type_name).ok_or_else(|| {
            err_msg(
                IndyErrorKind::UnknownCrypto,
                format!(
                    "Trying to verify message with unknown crypto: {}",
                    crypto_type_name
                ),
            )
        })?;

        let their_vk = ed25519_sign::PublicKey::from_slice(&their_vk.from_base58()?)?;
        let signature = ed25519_sign::Signature::from_slice(&signature)?;

        let valid = crypto_type.verify(&their_vk, msg, &signature)?;

        let res = Ok(valid);
        trace!("verify < {:?}", res);
        res
    }

    pub(crate) async fn crypto_box(
        &self,
        my_key: &Key,
        their_vk: &str,
        doc: &[u8],
    ) -> IndyResult<(Vec<u8>, Vec<u8>)> {
        trace!(
            "crypto_box > my_key {:?} their_vk {:?} doc {:?}",
            my_key,
            their_vk,
            doc
        );

        let crypto_type_name = verkey_get_cryptoname(&my_key.verkey);

        let (their_vk, their_crypto_type_name) = split_verkey(their_vk);

        if !crypto_type_name.eq(their_crypto_type_name) {
            // TODO: FIXME: Use dedicated error code
            return Err(err_msg(
                IndyErrorKind::UnknownCrypto,
                format!(
                    "My key crypto type is incompatible with their key crypto type: {} {}",
                    crypto_type_name, their_crypto_type_name
                ),
            ));
        }

        let crypto_types = self.crypto_types.read().await;

        let crypto_type = crypto_types.get(crypto_type_name).ok_or_else(|| {
            err_msg(
                IndyErrorKind::UnknownCrypto,
                format!(
                    "Trying to crypto_box message with unknown crypto: {}",
                    crypto_type_name
                ),
            )
        })?;

        let my_sk =
            ed25519_sign::SecretKey::from_slice(my_key.signkey.as_str().from_base58()?.as_slice())?;

        let their_vk = ed25519_sign::PublicKey::from_slice(their_vk.from_base58()?.as_slice())?;
        let nonce = crypto_type.gen_nonce();

        let encrypted_doc = crypto_type.crypto_box(&my_sk, &their_vk, doc, &nonce)?;
        let nonce = nonce[..].to_vec();

        let res = Ok((encrypted_doc, nonce));
        trace!("crypto_box < {:?}", res);
        res
    }

    pub(crate) async fn crypto_box_open(
        &self,
        my_key: &Key,
        their_vk: &str,
        doc: &[u8],
        nonce: &[u8],
    ) -> IndyResult<Vec<u8>> {
        trace!(
            "crypto_box_open > my_key {:?} their_vk {:?} doc {:?} nonce {:?}",
            my_key,
            their_vk,
            doc,
            nonce
        );

        let crypto_type_name = verkey_get_cryptoname(&my_key.verkey);
        let (their_vk, their_crypto_type_name) = split_verkey(their_vk);

        if !crypto_type_name.eq(their_crypto_type_name) {
            // TODO: FIXME: Use dedicated error code
            return Err(err_msg(
                IndyErrorKind::UnknownCrypto,
                format!(
                    "My key crypto type is incompatible with their key crypto type: {} {}",
                    crypto_type_name, their_crypto_type_name
                ),
            ));
        }

        let crypto_types = self.crypto_types.read().await;

        let crypto_type = crypto_types.get(crypto_type_name).ok_or_else(|| {
            err_msg(
                IndyErrorKind::UnknownCrypto,
                format!(
                    "Trying to crypto_box_open message with unknown crypto: {}",
                    crypto_type_name
                ),
            )
        })?;

        let my_sk = ed25519_sign::SecretKey::from_slice(&my_key.signkey.from_base58()?.as_slice())?;
        let their_vk = ed25519_sign::PublicKey::from_slice(their_vk.from_base58()?.as_slice())?;
        let nonce = ed25519_box::Nonce::from_slice(&nonce)?;

        let decrypted_doc = crypto_type.crypto_box_open(&my_sk, &their_vk, &doc, &nonce)?;

        let res = Ok(decrypted_doc);
        trace!("crypto_box_open < {:?}", res);
        res
    }

    pub(crate) async fn crypto_box_seal(&self, their_vk: &str, doc: &[u8]) -> IndyResult<Vec<u8>> {
        trace!("crypto_box_seal > their_vk {:?} doc {:?}", their_vk, doc);

        let (their_vk, crypto_type_name) = split_verkey(their_vk);
        let crypto_types = self.crypto_types.read().await;

        let crypto_type = crypto_types.get(crypto_type_name).ok_or_else(|| {
            err_msg(
                IndyErrorKind::UnknownCrypto,
                format!(
                    "Trying to encrypt sealed message with unknown crypto: {}",
                    crypto_type_name
                ),
            )
        })?;

        let their_vk = ed25519_sign::PublicKey::from_slice(their_vk.from_base58()?.as_slice())?;
        let encrypted_doc = crypto_type.crypto_box_seal(&their_vk, doc)?;

        let res = Ok(encrypted_doc);
        trace!("crypto_box_seal < {:?}", res);
        res
    }

    pub(crate) async fn crypto_box_seal_open(
        &self,
        my_key: &Key,
        doc: &[u8],
    ) -> IndyResult<Vec<u8>> {
        trace!("crypto_box_seal_open > my_key {:?} doc {:?}", my_key, doc);

        let (my_vk, crypto_type_name) = split_verkey(&my_key.verkey);

        let crypto_types = self.crypto_types.read().await;

        let crypto_type = crypto_types.get(crypto_type_name).ok_or_else(|| {
            err_msg(
                IndyErrorKind::UnknownCrypto,
                format!(
                    "Trying to crypto_box_open sealed message with unknown crypto: {}",
                    crypto_type_name
                ),
            )
        })?;

        let my_vk = ed25519_sign::PublicKey::from_slice(my_vk.from_base58()?.as_slice())?;

        let my_sk =
            ed25519_sign::SecretKey::from_slice(my_key.signkey.as_str().from_base58()?.as_slice())?;

        let decrypted_doc = crypto_type.crypto_box_seal_open(&my_vk, &my_sk, doc)?;

        let res = Ok(decrypted_doc);
        trace!("crypto_box_seal_open < {:?}", res);
        res
    }

    pub(crate) fn convert_seed(
        &self,
        seed: Option<&str>,
    ) -> IndyResult<Option<ed25519_sign::Seed>> {
        trace!("convert_seed > seed {:?}", secret!(seed));

        if seed.is_none() {
            trace!("convert_seed <<< res: None");
            return Ok(None);
        }

        let seed = seed.unwrap();

        let bytes = if seed.as_bytes().len() == ed25519_sign::SEEDBYTES {
            // is acceptable seed length
            seed.as_bytes().to_vec()
        } else if seed.ends_with('=') {
            // is base64 string
            let decoded = base64::decode(&seed).to_indy(
                IndyErrorKind::InvalidStructure,
                "Can't deserialize Seed from Base64 string",
            )?;
            if decoded.len() == ed25519_sign::SEEDBYTES {
                decoded
            } else {
                return Err(err_msg(
                    IndyErrorKind::InvalidStructure,
                    format!(
                        "Trying to use invalid base64 encoded `seed`. \
                                   The number of bytes must be {} ",
                        ed25519_sign::SEEDBYTES
                    ),
                ));
            }
        } else if seed.as_bytes().len() == ed25519_sign::SEEDBYTES * 2 {
            // is hex string
            Vec::from_hex(seed).to_indy(IndyErrorKind::InvalidStructure, "Seed is invalid hex")?
        } else {
            return Err(err_msg(
                IndyErrorKind::InvalidStructure,
                format!(
                    "Trying to use invalid `seed`. It can be either \
                               {} bytes string or base64 string or {} bytes HEX string",
                    ed25519_sign::SEEDBYTES,
                    ed25519_sign::SEEDBYTES * 2
                ),
            ));
        };

        let seed = ed25519_sign::Seed::from_slice(bytes.as_slice())?;

        let res = Ok(Some(seed));
        trace!("convert_seed < {:?}", secret!(&res));
        res
    }

    pub(crate) async fn validate_key(&self, vk: &str) -> IndyResult<()> {
        trace!("validate_key > vk {:?}", vk);

        let (vk, crypto_type_name) = split_verkey(vk);

        let crypto_types = self.crypto_types.read().await;

        let crypto_type = crypto_types.get(crypto_type_name).ok_or_else(|| {
            err_msg(
                IndyErrorKind::UnknownCrypto,
                format!(
                    "Trying to use key with unknown crypto: {}",
                    crypto_type_name
                ),
            )
        })?;

        if vk.starts_with('~') {
            let _ = vk[1..].from_base58()?; // TODO: proper validate abbreviated verkey
        } else {
            let vk = ed25519_sign::PublicKey::from_slice(vk.from_base58()?.as_slice())?;
            crypto_type.validate_key(&vk)?;
        };

        let res = Ok(());
        trace!("validate_key < {:?}", res);
        res
    }

    pub(crate) fn validate_did(&self, did: &DidValue) -> IndyResult<()> {
        trace!("validate_did > did {:?}", did);
        // Useful method, huh?
        // Soon some state did validation will be put here

        let res = Ok(());
        trace!("validate_did < {:?}", res);
        res
    }

    pub(crate) fn validate_opt_did(&self, did: Option<&DidValue>) -> IndyResult<()> {
        match did {
            Some(did) => Ok(self.validate_did(did)?),
            None => Ok(()),
        }
    }

    pub(crate) fn encrypt_plaintext(
        &self,
        plaintext: Vec<u8>,
        aad: &str,
        cek: &chacha20poly1305_ietf::Key,
    ) -> (String, String, String) {
        //encrypt message with aad
        let (ciphertext, iv, tag) =
            gen_nonce_and_encrypt_detached(plaintext.as_slice(), aad.as_bytes(), &cek);

        //base64 url encode data
        let iv_encoded = base64::encode_urlsafe(&iv[..]);
        let ciphertext_encoded = base64::encode_urlsafe(ciphertext.as_slice());
        let tag_encoded = base64::encode_urlsafe(&tag[..]);

        (ciphertext_encoded, iv_encoded, tag_encoded)
    }

    /* ciphertext helper functions*/
    pub(crate) fn decrypt_ciphertext(
        &self,
        ciphertext: &str,
        aad: &str,
        iv: &str,
        tag: &str,
        cek: &chacha20poly1305_ietf::Key,
    ) -> Result<String, IndyError> {
        //convert ciphertext to bytes
        let ciphertext_as_vec = base64::decode_urlsafe(ciphertext).map_err(|err| {
            err_msg(
                IndyErrorKind::InvalidStructure,
                format!("Failed to decode ciphertext {}", err),
            )
        })?;

        let ciphertext_as_bytes = ciphertext_as_vec.as_ref();

        //convert IV from &str to &Nonce
        let nonce_as_vec = base64::decode_urlsafe(iv).map_err(|err| {
            err_msg(
                IndyErrorKind::InvalidStructure,
                format!("Failed to decode IV {}", err),
            )
        })?;

        let nonce_as_slice = nonce_as_vec.as_slice();

        let nonce = chacha20poly1305_ietf::Nonce::from_slice(nonce_as_slice).map_err(|err| {
            err_msg(
                IndyErrorKind::InvalidStructure,
                format!("Failed to convert IV to Nonce type {}", err),
            )
        })?;

        //convert tag from &str to &Tag
        let tag_as_vec = base64::decode_urlsafe(tag).map_err(|err| {
            err_msg(
                IndyErrorKind::InvalidStructure,
                format!("Failed to decode tag {}", err),
            )
        })?;
        let tag_as_slice = tag_as_vec.as_slice();
        let tag = chacha20poly1305_ietf::Tag::from_slice(tag_as_slice).map_err(|err| {
            err_msg(
                IndyErrorKind::InvalidStructure,
                format!("Failed to convert tag to Tag type {}", err),
            )
        })?;

        //decrypt message
        let plaintext_bytes = chacha20poly1305_ietf::decrypt_detached(
            ciphertext_as_bytes,
            cek,
            &nonce,
            &tag,
            Some(aad.as_bytes()),
        )
        .map_err(|err| {
            err_msg(
                IndyErrorKind::UnknownCrypto,
                format!("Failed to decrypt ciphertext {}", err),
            )
        })?;

        //convert message to readable (UTF-8) string
        String::from_utf8(plaintext_bytes).map_err(|err| {
            err_msg(
                IndyErrorKind::InvalidStructure,
                format!("Failed to convert message to UTF-8 {}", err),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::domain::crypto::did::MyDidInfo;
    use indy_utils::crypto::chacha20poly1305_ietf::gen_key;

    use super::*;

    #[async_std::test]
    async fn create_my_did_with_works_for_empty_info() {
        let service = CryptoService::new();
        let did_info = MyDidInfo {
            did: None,
            cid: None,
            seed: None,
            crypto_type: None,
            method_name: None,
            ledger_type: None,
        };
        let my_did = service.create_my_did(&did_info).await;
        assert!(my_did.is_ok());
    }

    #[async_std::test]
    async fn create_my_did_works_for_passed_did() {
        let service = CryptoService::new();

        let did = DidValue("NcYxiDXkpYi6ov5FcYDi1e".to_string());
        let did_info = MyDidInfo {
            did: Some(did.clone()),
            cid: None,
            seed: None,
            crypto_type: None,
            method_name: None,
            ledger_type: None,
        };

        let (my_did, _) = service.create_my_did(&did_info).await.unwrap();
        assert_eq!(did, my_did.did);
    }

    #[async_std::test]
    async fn create_my_did_not_works_for_invalid_crypto_type() {
        let service = CryptoService::new();

        let did = DidValue("NcYxiDXkpYi6ov5FcYDi1e".to_string());
        let crypto_type = Some("type".to_string());

        let did_info = MyDidInfo {
            did: Some(did),
            cid: None,
            seed: None,
            crypto_type,
            method_name: None,
            ledger_type: None,
        };

        assert!(service.create_my_did(&did_info).await.is_err());
    }

    #[async_std::test]
    async fn create_my_did_works_for_seed() {
        let service = CryptoService::new();

        let did = DidValue("NcYxiDXkpYi6ov5FcYDi1e".to_string());
        let seed = Some("00000000000000000000000000000My1".to_string());

        let did_info_with_seed = MyDidInfo {
            did: Some(did.clone()),
            cid: None,
            seed,
            crypto_type: None,
            method_name: None,
            ledger_type: None,
        };
        let did_info_without_seed = MyDidInfo {
            did: Some(did.clone()),
            cid: None,
            seed: None,
            crypto_type: None,
            method_name: None,
            ledger_type: None,
        };

        let (did_with_seed, _) = service.create_my_did(&did_info_with_seed).await.unwrap();
        let (did_without_seed, _) = service.create_my_did(&did_info_without_seed).await.unwrap();

        assert_ne!(did_with_seed.verkey, did_without_seed.verkey)
    }

    #[async_std::test]
    async fn create_their_did_works_without_verkey() {
        let service = CryptoService::new();
        let did = DidValue("CnEDk9HrMnmiHXEV1WFgbVCRteYnPqsJwrTdcZaNhFVW".to_string());

        let their_did_info = TheirDidInfo::new(did.clone(), None);
        let their_did = service.create_their_did(&their_did_info).await.unwrap();

        assert_eq!(did, their_did.did);
        assert_eq!(did.0, their_did.verkey);
    }

    #[async_std::test]
    async fn create_their_did_works_for_full_verkey() {
        let service = CryptoService::new();
        let did = DidValue("8wZcEriaNLNKtteJvx7f8i".to_string());
        let verkey = "5L2HBnzbu6Auh2pkDRbFt5f4prvgE2LzknkuYLsKkacp";

        let their_did_info = TheirDidInfo::new(did.clone(), Some(verkey.to_string()));
        let their_did = service.create_their_did(&their_did_info).await.unwrap();

        assert_eq!(did, their_did.did);
        assert_eq!(verkey, their_did.verkey);
    }

    #[async_std::test]
    async fn create_their_did_works_for_abbreviated_verkey() {
        let service = CryptoService::new();
        let did = DidValue("8wZcEriaNLNKtteJvx7f8i".to_string());

        let their_did_info =
            TheirDidInfo::new(did.clone(), Some("~NcYxiDXkpYi6ov5FcYDi1e".to_string()));

        let their_did = service.create_their_did(&their_did_info).await.unwrap();

        assert_eq!(did, their_did.did);

        assert_eq!(
            "5L2HBnzbu6Auh2pkDRbFt5f4prvgE2LzknkuYLsKkacp",
            their_did.verkey
        );
    }

    #[async_std::test]
    async fn sign_works() {
        let service = CryptoService::new();
        let did_info = MyDidInfo {
            did: None,
            cid: None,
            seed: None,
            crypto_type: None,
            method_name: None,
            ledger_type: None,
        };

        let message = r#"message"#;
        let (_, my_key) = service.create_my_did(&did_info).await.unwrap();
        let sig = service.sign(&my_key, message.as_bytes()).await;

        assert!(sig.is_ok());
    }

    #[async_std::test]
    async fn sign_works_for_invalid_signkey() {
        let service = CryptoService::new();
        let message = r#"message"#;
        let my_key = Key::new(
            "8wZcEriaNLNKtteJvx7f8i".to_string(),
            "5L2HBnzbu6Auh2pkDRbFt5f4prvgE2LzknkuYLsKkacp".to_string(),
        );
        assert!(service.sign(&my_key, message.as_bytes()).await.is_err());
    }

    #[async_std::test]
    async fn sign_verify_works() {
        let service = CryptoService::new();
        let did_info = MyDidInfo {
            did: None,
            cid: None,
            seed: None,
            crypto_type: None,
            method_name: None,
            ledger_type: None,
        };
        let message = r#"message"#;
        let (my_did, my_key) = service.create_my_did(&did_info).await.unwrap();
        let signature = service.sign(&my_key, message.as_bytes()).await.unwrap();

        let valid = service
            .verify(&my_did.verkey, message.as_bytes(), &signature)
            .await
            .unwrap();

        assert!(valid);
    }

    #[async_std::test]
    async fn sign_verify_works_for_verkey_contained_crypto_type() {
        let service = CryptoService::new();
        let did_info = MyDidInfo {
            did: None,
            cid: None,
            seed: None,
            crypto_type: None,
            method_name: None,
            ledger_type: None,
        };
        let message = r#"message"#;
        let (my_did, my_key) = service.create_my_did(&did_info).await.unwrap();
        let signature = service.sign(&my_key, message.as_bytes()).await.unwrap();
        let verkey = my_did.verkey + ":ed25519";
        let valid = service
            .verify(&verkey, message.as_bytes(), &signature)
            .await
            .unwrap();
        assert!(valid);
    }

    #[async_std::test]
    async fn sign_verify_works_for_verkey_contained_invalid_crypto_type() {
        let service = CryptoService::new();

        let did_info = MyDidInfo {
            did: None,
            cid: None,
            seed: None,
            crypto_type: None,
            method_name: None,
            ledger_type: None,
        };

        let message = r#"message"#;
        let (my_did, my_key) = service.create_my_did(&did_info).await.unwrap();
        let signature = service.sign(&my_key, message.as_bytes()).await.unwrap();
        let verkey = format!("crypto_type:{}", my_did.verkey);

        assert!(service
            .verify(&verkey, message.as_bytes(), &signature)
            .await
            .is_err());
    }

    #[async_std::test]
    async fn verify_not_works_for_invalid_verkey() {
        let service = CryptoService::new();

        let did_info = MyDidInfo {
            did: None,
            cid: None,
            seed: None,
            crypto_type: None,
            method_name: None,
            ledger_type: None,
        };
        let message = r#"message"#;
        let (_, my_key) = service.create_my_did(&did_info).await.unwrap();
        let signature = service.sign(&my_key, message.as_bytes()).await.unwrap();
        let verkey = "AnnxV4t3LUHKZaxVQDWoVaG44NrGmeDYMA4Gz6C2tCZd";

        let valid = service
            .verify(verkey, message.as_bytes(), &signature)
            .await
            .unwrap();

        assert_eq!(false, valid);
    }

    #[async_std::test]
    async fn crypto_box_works() {
        let service = CryptoService::new();
        let msg = "some message";

        let did_info = MyDidInfo {
            did: None,
            cid: None,
            seed: None,
            crypto_type: None,
            method_name: None,
            ledger_type: None,
        };

        let (_, my_key) = service.create_my_did(&did_info).await.unwrap();
        let (their_did, _) = service.create_my_did(&did_info.clone()).await.unwrap();
        let their_did = Did::new(their_did.did, their_did.verkey);

        let encrypted_message = service
            .crypto_box(&my_key, &their_did.verkey, msg.as_bytes())
            .await;

        assert!(encrypted_message.is_ok());
    }

    #[async_std::test]
    async fn crypto_box_and_crypto_box_open_works() {
        let service = CryptoService::new();

        let msg = "some message";

        let did_info = MyDidInfo {
            did: None,
            cid: None,
            seed: None,
            crypto_type: None,
            method_name: None,
            ledger_type: None,
        };

        let (my_did, my_key) = service.create_my_did(&did_info).await.unwrap();

        let my_key_for_encrypt = my_key.clone();

        let their_did_for_decrypt = Did::new(my_did.did, my_did.verkey);

        let (their_did, their_key) = service.create_my_did(&did_info.clone()).await.unwrap();

        let my_key_for_decrypt = their_key.clone();

        let their_did_for_encrypt = Did::new(their_did.did, their_did.verkey);

        let (encrypted_message, noce) = service
            .crypto_box(
                &my_key_for_encrypt,
                &their_did_for_encrypt.verkey,
                msg.as_bytes(),
            )
            .await
            .unwrap();

        let decrypted_message = service
            .crypto_box_open(
                &my_key_for_decrypt,
                &their_did_for_decrypt.verkey,
                &encrypted_message,
                &noce,
            )
            .await
            .unwrap();

        assert_eq!(msg.as_bytes().to_vec(), decrypted_message);
    }

    #[async_std::test]
    async fn crypto_box_and_crypto_box_open_works_for_verkey_contained_crypto_type() {
        let service = CryptoService::new();

        let msg = "some message";

        let did_info = MyDidInfo {
            did: None,
            cid: None,
            seed: None,
            crypto_type: None,
            method_name: None,
            ledger_type: None,
        };

        let (my_did, my_key) = service.create_my_did(&did_info).await.unwrap();

        let my_key_for_encrypt = my_key.clone();

        let their_did_for_decrypt = Did::new(my_did.did, my_did.verkey);

        let (their_did, their_key) = service.create_my_did(&did_info.clone()).await.unwrap();
        let my_key_for_decrypt = their_key.clone();

        let their_did_for_encrypt = Did::new(their_did.did, their_did.verkey);

        let (encrypted_message, noce) = service
            .crypto_box(
                &my_key_for_encrypt,
                &their_did_for_encrypt.verkey,
                msg.as_bytes(),
            )
            .await
            .unwrap();

        let verkey = their_did_for_decrypt.verkey + ":ed25519";

        let decrypted_message = service
            .crypto_box_open(&my_key_for_decrypt, &verkey, &encrypted_message, &noce)
            .await
            .unwrap();

        assert_eq!(msg.as_bytes().to_vec(), decrypted_message);
    }

    #[async_std::test]
    async fn crypto_box_seal_works() {
        let service = CryptoService::new();
        let msg = "some message";
        let did_info = MyDidInfo {
            did: None,
            cid: None,
            seed: None,
            crypto_type: None,
            method_name: None,
            ledger_type: None,
        };
        let (did, _) = service.create_my_did(&did_info.clone()).await.unwrap();
        let did = Did::new(did.did, did.verkey);
        let encrypted_message = service.crypto_box_seal(&did.verkey, msg.as_bytes()).await;
        assert!(encrypted_message.is_ok());
    }

    #[async_std::test]
    async fn crypto_box_seal_and_crypto_box_seal_open_works() {
        let service = CryptoService::new();
        let msg = "some message".as_bytes();

        let did_info = MyDidInfo {
            did: None,
            cid: None,
            seed: None,
            crypto_type: None,
            method_name: None,
            ledger_type: None,
        };

        let (did, key) = service.create_my_did(&did_info.clone()).await.unwrap();
        let encrypt_did = Did::new(did.did.clone(), did.verkey.clone());

        let encrypted_message = service
            .crypto_box_seal(&encrypt_did.verkey, msg)
            .await
            .unwrap();

        let decrypted_message = service
            .crypto_box_seal_open(&key, &encrypted_message)
            .await
            .unwrap();

        assert_eq!(msg, decrypted_message.as_slice());
    }

    #[async_std::test]
    async fn test_encrypt_plaintext_and_decrypt_ciphertext_works() {
        let service: CryptoService = CryptoService::new();
        let plaintext = "Hello World".as_bytes().to_vec();
        // AAD allows the sender to tie extra (protocol) data to the encryption. Example JWE enc and alg
        // Which the receiver MUST then check before decryption
        let aad = "some protocol data input to the encryption";
        let cek = gen_key();

        let (expected_ciphertext, iv_encoded, tag) =
            service.encrypt_plaintext(plaintext.clone(), aad, &cek);

        let expected_plaintext = service
            .decrypt_ciphertext(&expected_ciphertext, aad, &iv_encoded, &tag, &cek)
            .unwrap();

        assert_eq!(expected_plaintext.as_bytes().to_vec(), plaintext);
    }

    #[async_std::test]
    async fn test_encrypt_plaintext_decrypt_ciphertext_empty_string_works() {
        let service: CryptoService = CryptoService::new();
        let plaintext = "".as_bytes().to_vec();
        // AAD allows the sender to tie extra (protocol) data to the encryption. Example JWE enc and alg
        // Which the receiver MUST then check before decryption
        let aad = "some protocol data input to the encryption";
        let cek = gen_key();

        let (expected_ciphertext, iv_encoded, tag) =
            service.encrypt_plaintext(plaintext.clone(), aad, &cek);

        let expected_plaintext = service
            .decrypt_ciphertext(&expected_ciphertext, aad, &iv_encoded, &tag, &cek)
            .unwrap();

        assert_eq!(expected_plaintext.as_bytes().to_vec(), plaintext);
    }

    #[async_std::test]
    async fn test_encrypt_plaintext_decrypt_ciphertext_bad_iv_fails() {
        let service: CryptoService = CryptoService::new();
        let plaintext = "Hello World".as_bytes().to_vec();
        // AAD allows the sender to tie extra (protocol) data to the encryption. Example JWE enc and alg
        // Which the receiver MUST then check before decryption
        let aad = "some protocol data input to the encryption";
        let cek = gen_key();

        let (expected_ciphertext, _, tag) = service.encrypt_plaintext(plaintext, aad, &cek);

        //convert values to base64 encoded strings
        let bad_iv_input = "invalid_iv";

        let expected_error =
            service.decrypt_ciphertext(&expected_ciphertext, bad_iv_input, &tag, aad, &cek);

        assert!(expected_error.is_err());
    }

    #[async_std::test]
    async fn test_encrypt_plaintext_decrypt_ciphertext_bad_ciphertext_fails() {
        let service: CryptoService = CryptoService::new();
        let plaintext = "Hello World".as_bytes().to_vec();
        // AAD allows the sender to tie extra (protocol) data to the encryption. Example JWE enc and alg
        // Which the receiver MUST then check before decryption
        let aad = "some protocol data input to the encryption";
        let cek = gen_key();

        let (_, iv_encoded, tag) = service.encrypt_plaintext(plaintext, aad, &cek);

        let bad_ciphertext = base64::encode_urlsafe("bad_ciphertext".as_bytes());

        let expected_error =
            service.decrypt_ciphertext(&bad_ciphertext, &iv_encoded, &tag, aad, &cek);

        assert!(expected_error.is_err());
    }

    #[async_std::test]
    async fn test_encrypt_plaintext_and_decrypt_ciphertext_wrong_cek_fails() {
        let service: CryptoService = CryptoService::new();
        let plaintext = "Hello World".as_bytes().to_vec();
        // AAD allows the sender to tie extra (protocol) data to the encryption. Example JWE enc and alg
        // Which the receiver MUST then check before decryption
        let aad = "some protocol data input to the encryption";
        let cek = chacha20poly1305_ietf::gen_key();

        let (expected_ciphertext, iv_encoded, tag) =
            service.encrypt_plaintext(plaintext, aad, &cek);

        let bad_cek = gen_key();

        let expected_error =
            service.decrypt_ciphertext(&expected_ciphertext, &iv_encoded, &tag, aad, &bad_cek);

        assert!(expected_error.is_err());
    }

    #[async_std::test]
    async fn test_encrypt_plaintext_and_decrypt_ciphertext_bad_tag_fails() {
        let service: CryptoService = CryptoService::new();
        let plaintext = "Hello World".as_bytes().to_vec();
        // AAD allows the sender to tie extra (protocol) data to the encryption. Example JWE enc and alg
        // Which the receiver MUST then check before decryption
        let aad = "some protocol data input to the encryption";
        let cek = gen_key();

        let (expected_ciphertext, iv_encoded, _) = service.encrypt_plaintext(plaintext, aad, &cek);

        let bad_tag = "bad_tag".to_string();

        let expected_error =
            service.decrypt_ciphertext(&expected_ciphertext, &iv_encoded, &bad_tag, aad, &cek);
        assert!(expected_error.is_err());
    }

    #[async_std::test]
    async fn test_encrypt_plaintext_and_decrypt_ciphertext_bad_aad_fails() {
        let service: CryptoService = CryptoService::new();
        let plaintext = "Hello World".as_bytes().to_vec();
        // AAD allows the sender to tie extra (protocol) data to the encryption. Example JWE enc and alg
        // Which the receiver MUST then check before decryption
        let aad = "some protocol data input to the encryption";
        let cek = gen_key();

        let (expected_ciphertext, iv_encoded, tag) =
            service.encrypt_plaintext(plaintext, aad, &cek);

        let bad_aad = "bad aad";

        let expected_error =
            service.decrypt_ciphertext(&expected_ciphertext, &iv_encoded, &tag, bad_aad, &cek);
        assert!(expected_error.is_err());
    }
}
