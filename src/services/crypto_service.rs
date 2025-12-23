//! Service de cryptographie utilisant AES-GCM-256.
//!
//! Ce module fournit des fonctions pour chiffrer et déchiffrer des données
//! de manière sécurisée en utilisant un chiffrement authentifié (AEAD).
//! Chaque chiffrement génère un nonce unique de 96 bits qui est préfixé au message.

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng, AeadCore},
    Aes256Gcm, Key
};
use crate::error::AppError;

/// Taille du nonce pour AES-GCM (standard: 12 octets / 96 bits).
const NONCE_SIZE: usize = 12;

/// Chiffre un texte en clair avec une clé de 256 bits.
///
/// # Arguments
/// * `plaintext` - Le texte à chiffrer.
/// * `key` - Une tranche d'octets de 32 octets (256 bits).
///
/// # Returns
/// * `Ok(Vec<u8>)` - Un vecteur contenant le nonce suivi du ciphertext authentifié.
/// * `Err(AppError)` - En cas d'échec du chiffrement.
///
/// # Panics
/// Panique si la taille de la clé n'est pas exactement de 32 octets.
///
/// # Security
/// Cette fonction utilise `OsRng` pour garantir un nonce unique à chaque appel.
/// Ne jamais réutiliser la même combinaison (Clé, Nonce) pour deux messages différents.
///
/// # Examples
/// ```
/// # use hangar_back::services::crypto_service::{encrypt, decrypt};
/// let key = [0u8; 32]; // Exemple uniquement, utilisez une vraie clé
/// let encrypted = encrypt("hello", &key).unwrap();
/// let decrypted = decrypt(&encrypted, &key).unwrap();
/// assert_eq!(decrypted, "hello");
/// ```
pub fn encrypt(plaintext: &str, key: &[u8]) -> Result<Vec<u8>, AppError>
{
    let key: &Key<Aes256Gcm> = key.into();
    let cipher = Aes256Gcm::new(key);
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext = cipher.encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e|
        {
            tracing::error!("Encryption failed: {}", e);
            AppError::InternalServerError
        })?;
    
    let mut result = Vec::with_capacity(NONCE_SIZE + ciphertext.len());
    result.extend_from_slice(&nonce);
    result.extend_from_slice(&ciphertext);

    Ok(result)
}

/// Déchiffre un message préalablement chiffré par [`encrypt`].
///
/// # Arguments
/// * `ciphertext_with_nonce` - Les données brutes (12 octets de nonce + ciphertext).
/// * `key` - La clé de 32 octets utilisée pour le chiffrement.
///
/// # Errors
/// Retourne une `AppError::InternalServerError` si :
/// * Les données sont trop courtes pour contenir un nonce.
/// * La clé est incorrecte.
/// * Les données ont été corrompues (échec de l'authentification GCM).
/// * Le contenu déchiffré n'est pas de l'UTF-8 valide.
pub fn decrypt(ciphertext_with_nonce: &[u8], key: &[u8]) -> Result<String, AppError>
{
    if ciphertext_with_nonce.len() < NONCE_SIZE
    {
        tracing::error!("Ciphertext is too short to contain a nonce.");
        return Err(AppError::InternalServerError);
    }

    let key: &Key<Aes256Gcm> = key.into();
    let cipher = Aes256Gcm::new(key);

    let (nonce_bytes, ciphertext) = ciphertext_with_nonce.split_at(NONCE_SIZE);
    let nonce = nonce_bytes.into();

    let plaintext_bytes = cipher.decrypt(nonce, ciphertext)
        .map_err(|e|
        {
            tracing::error!("Decryption failed: {}. This might happen if the key is wrong or the data is corrupted.", e);
            AppError::InternalServerError
        })?;

    String::from_utf8(plaintext_bytes)
        .map_err(|_| AppError::InternalServerError)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Génère une clé de test valide (32 octets).
    fn test_key() -> Vec<u8> 
    {
        vec![0x42; 32]
    }

    /// Génère une clé différente pour les tests de mauvaise clé.
    fn wrong_key() -> Vec<u8> 
    {
        vec![0xFF; 32]
    }


    #[test]
    fn test_encrypt_decrypt_roundtrip() 
    {
        let key = test_key();
        let plaintext = "Mon secret ultra confidentiel";

        let encrypted = encrypt(plaintext, &key).expect("Encryption failed");
        let decrypted = decrypt(&encrypted, &key).expect("Decryption failed");

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_empty_string() 
    {
        let key = test_key();
        let plaintext = "";

        let encrypted = encrypt(plaintext, &key).expect("Encryption of empty string failed");
        assert!(encrypted.len() >= NONCE_SIZE); // Nonce + tag minimum

        let decrypted = decrypt(&encrypted, &key).expect("Decryption failed");
        assert_eq!(decrypted, "");
    }

    #[test]
    fn test_encrypt_unicode_characters() 
    {
        let key = test_key();
        let plaintext = "Héllo Wörld! 你好 مرحبا 🌍";

        let encrypted = encrypt(plaintext, &key).expect("Encryption failed");
        let decrypted = decrypt(&encrypted, &key).expect("Decryption failed");

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_long_text() 
    {
        let key = test_key();
        let plaintext = "A".repeat(10_000); // 10 KB de données

        let encrypted = encrypt(&plaintext, &key).expect("Encryption failed");
        let decrypted = decrypt(&encrypted, &key).expect("Decryption failed");

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypted_output_contains_nonce() 
    {
        let key = test_key();
        let plaintext = "test";

        let encrypted = encrypt(plaintext, &key).expect("Encryption failed");

        // Le résultat doit contenir au moins : nonce (12) + texte chiffré + tag (16)
        assert!(encrypted.len() >= NONCE_SIZE + 16);
    }

    #[test]
    fn test_nonce_is_random() 
    {
        let key = test_key();
        let plaintext = "same text";

        let encrypted1 = encrypt(plaintext, &key).expect("Encryption 1 failed");
        let encrypted2 = encrypt(plaintext, &key).expect("Encryption 2 failed");

        // Les deux chiffrements doivent être différents (nonce aléatoire)
        assert_ne!(encrypted1, encrypted2);

        // Mais les deux doivent déchiffrer au même résultat
        assert_eq!(decrypt(&encrypted1, &key).unwrap(), plaintext);
        assert_eq!(decrypt(&encrypted2, &key).unwrap(), plaintext);
    }

    #[test]
    fn test_decrypt_with_wrong_key() 
    {
        let correct_key = test_key();
        let wrong_key = wrong_key();
        let plaintext = "secret";

        let encrypted = encrypt(plaintext, &correct_key).expect("Encryption failed");

        // Le déchiffrement avec la mauvaise clé doit échouer
        let result = decrypt(&encrypted, &wrong_key);
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_too_short_data()
    {
        let key = test_key();
        let invalid_data = vec![0u8; 8]; // Moins de 12 octets

        let result = decrypt(&invalid_data, &key);
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_exactly_nonce_size() 
    {
        let key = test_key();
        let invalid_data = vec![0u8; NONCE_SIZE]; // Exactement 12 octets (nonce seul)

        // Devrait échouer car pas de ciphertext après le nonce
        let result = decrypt(&invalid_data, &key);
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_corrupted_ciphertext() 
    {
        let key = test_key();
        let plaintext = "secret";

        let mut encrypted = encrypt(plaintext, &key).expect("Encryption failed");

        // Corrompre un octet du ciphertext (après le nonce)
        if encrypted.len() > NONCE_SIZE {
            encrypted[NONCE_SIZE] ^= 0xFF;
        }

        // Le déchiffrement doit échouer (AEAD integrity check)
        let result = decrypt(&encrypted, &key);
        assert!(result.is_err());
    }

    #[test]
    fn test_decrypt_corrupted_nonce() 
    {
        let key = test_key();
        let plaintext = "secret";

        let mut encrypted = encrypt(plaintext, &key).expect("Encryption failed");

        // Corrompre le nonce (premiers octets)
        encrypted[0] ^= 0xFF;

        // Le déchiffrement doit échouer
        let result = decrypt(&encrypted, &key);
        assert!(result.is_err());
    }

    #[test]
    fn test_encrypt_special_characters() 
    {
        let key = test_key();
        let plaintext = "Line1\nLine2\tTabbed\r\nWindows\0Null";

        let encrypted = encrypt(plaintext, &key).expect("Encryption failed");
        let decrypted = decrypt(&encrypted, &key).expect("Decryption failed");

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_multiple_encryptions_different_nonces() 
    {
        let key = test_key();
        let plaintext = "test";
        let iterations = 100;

        let mut encrypted_values = Vec::new();
        for _ in 0..iterations {
            let encrypted = encrypt(plaintext, &key).expect("Encryption failed");
            encrypted_values.push(encrypted);
        }

        // Vérifier que tous les nonces sont différents (très haute probabilité)
        let nonces: Vec<_> = encrypted_values
            .iter()
            .map(|e| &e[..NONCE_SIZE])
            .collect();

        let unique_nonces: std::collections::HashSet<_> = nonces.iter().collect();
        assert_eq!(unique_nonces.len(), iterations);
    }

    #[test]
    #[should_panic]
    fn test_encrypt_with_invalid_key_size() 
    {
        let invalid_key = vec![0u8; 16]; // 128 bits au lieu de 256
        let _ = encrypt("test", &invalid_key);
    }

    #[test]
    #[should_panic]
    fn test_decrypt_with_invalid_key_size() 
    {
        let invalid_key = vec![0u8; 16]; // 128 bits au lieu de 256
        let fake_data = vec![0u8; 32];
        let _ = decrypt(&fake_data, &invalid_key);
    }

    #[test]
    fn test_encrypt_decrypt_with_all_zero_key() 
    {
        let key = vec![0u8; 32];
        let plaintext = "Testing with zero key";

        let encrypted = encrypt(plaintext, &key).expect("Encryption failed");
        let decrypted = decrypt(&encrypted, &key).expect("Decryption failed");

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_decrypt_with_all_ones_key() 
    {
        let key = vec![0xFF; 32];
        let plaintext = "Testing with ones key";

        let encrypted = encrypt(plaintext, &key).expect("Encryption failed");
        let decrypted = decrypt(&encrypted, &key).expect("Decryption failed");

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_ciphertext_length_calculation() 
    {
        let key = test_key();
        let plaintext = "test message";

        let encrypted = encrypt(plaintext, &key).expect("Encryption failed");

        // Longueur = NONCE (12) + plaintext.len() + TAG (16)
        let expected_min_length = NONCE_SIZE + plaintext.len() + 16;
        assert!(encrypted.len() >= expected_min_length);
    }
}