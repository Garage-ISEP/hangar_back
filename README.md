# Hangar

[![Language: Rust](https://img.shields.io/badge/Language-Rust-orange.svg)](https://www.rust-lang.org/)
[![Docker](https://img.shields.io/badge/Platform-Docker-blue.svg)](https://www.docker.com/)
[![License: Custom](https://img.shields.io/badge/License-Open--Source%20with%20Credit-green.svg)](#license)

**Hangar** est la plateforme de déploiement automatisée conçue par la **DSI de Garage ISEP**. Elle permet aux étudiants de déployer instantanément des projets PHP/Web ou des images Docker, tout en gérant automatiquement le cycle de vie des conteneurs, le routage HTTPS et les bases de données.

Hangar a été creer à destination des élèves de l'Isep, école d'ingénieurs.

## 📖 Documentation Utilisateur

Retrouvez le guide complet (Premiers pas, déploiement GitHub, gestion des volumes et bonnes pratiques) ici :  
[**Documentation Hangar sur Outline**](https://outline.garageisep.com/s/6b296d0a-141c-4ca5-8551-de0da31880c7/doc/documentation-hangar-h2Ow69b9cQ)

## ✨ Fonctionnalités

- **Déploiement GitHub "One-Click"** : Liaison directe avec vos dépôts (publics ou privés) via une GitHub App.
- **Support Docker Avancé** : Déploiement direct depuis n'importe quelle image publique.
- **Base de Données MariaDB** : Provisionnement automatique d'une instance MariaDB par utilisateur.
- **Sécurité Native** :
    - Scan de vulnérabilités intégré avec **Grype**.
    - Chiffrement des variables d'environnement (AES-256-GCM).
    - Isolation stricte des conteneurs (AppArmor, No-root).
- **Zéro Downtime** : Processus de déploiement *Blue-Green* pour des mises à jour fluides.
- **Monitoring en Temps Réel** : Visualisation du CPU, de la RAM et flux de logs en direct.
- **HTTPS Automatique** : Gestion des certificats SSL via Traefik et Let's Encrypt.

## 🛠️ Stack Technique

- **Backend** : Rust (Axum/Tokio)
- **Runtime** : Docker Engine API
- **Reverse Proxy** : Traefik
- **Base de données Interne** : PostgreSQL
- **Base de données Utilisateurs** : MariaDB
- **Sécurité** : Anchore Grype
- **Image de base par défaut** : `nginx-php-base` (Alpine 3.22, PHP 8.4)

## 📋 Quotas et Limitations

Pour garantir la stabilité du serveur, les limites suivantes sont appliquées par défaut :
- **Utilisateur** : 1 projet et 1 base de données maximum.
- **CPU** : Limité à 50% d'un cœur (Quota 50000).
- **RAM** : 512 MiB par conteneur.
- **Processus** : Maximum 1024 PIDs.
- **Timeout** : 10s pour les requêtes standard / 300s pour les déploiements.

## 🤝 Contribution

Les contributions sont les bienvenues pour améliorer Hangar !
1. Forkez le projet.
2. Créez votre branche (`git checkout -b feature/AmazingFeature`).
3. Committez vos changements (`git commit -m 'Add some AmazingFeature'`).
4. Poussez vers la branche (`git push origin feature/AmazingFeature`).
5. Ouvrez une Pull Request.

## 📄 License

Ce projet est distribué sous une licence **Open-Source avec Crédit Obligatoire** :

1. **Usage Open-Source** : L'utilisation, la modification et la distribution de ce logiciel sont autorisées uniquement dans le cadre de projets open-source.
2. **Attribution** : Toute reprise du code, totale ou partielle, doit impérativement inclure une mention visible vers le projet original : `"Original project by Garage Isep (https://github.com/Garage-ISEP/hangar_back)"`.
3. **Usage Commercial** : L'utilisation commerciale est interdite sans autorisation préalable de la DSI de Garage Isep.

---
*Maintenu avec ❤️ par la DSI de [Garage Isep](https://garageisep.com).*