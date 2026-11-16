# Plan de réalisation — suivi

Ce suivi applique les besoins de [INSTRUCTIONS.md](INSTRUCTIONS.md) et le plan détaillé validé pendant la phase de conception.

## Objectif

Un serveur Rust en lecture seule expose une bibliothèque Batocera. Un client Rust/SDL2 permet de sélectionner les jeux à la manette, de télécharger ROMs et médias et de fusionner le `gamelist.xml` local sans perte des données existantes.

## Phase 0 — Cadrage

- [x] Arborescence et fixtures synthétiques documentées.
- [x] Contrat HTTP versionné et manifeste avec SHA-256.
- [x] Politique locale : refuser les conflits, ne jamais écraser une ROM différente.
- [ ] Confirmer version, architecture et mode de lancement Batocera sur le matériel cible.

## Phase 1 — Serveur et catalogue

- [x] Serveur Rust (Axum/Tokio), CLI Clap, configuration et tests Cargo.
- [x] Migration du serveur Python : API v1, identifiants/révisions et cache SQLite compatibles, ancien runtime retiré.
- [x] Lecture sécurisée des XML, chemins relatifs et médias.
- [x] Index SQLite publié en transaction, diagnostics et cache d'empreintes.
- [x] Logs de progression de l'indexation, état du cache, suivi SHA-256 des fichiers longs et bilan chronométré.
- [x] Réindexation à chaud par API, état asynchrone et regroupement des demandes simultanées.
- [x] Systèmes, jeux, recherche API, pagination et détails.
- [x] Isolation des entrées invalides et documentation OpenAPI.

## Phase 2 — Distribution

- [x] Manifestes versionnés et révisions de contenu.
- [x] Téléchargement par identifiants opaques, streaming et ETag / If-Match.
- [x] Refus des fichiers modifiés depuis l'indexation.
- [x] Tests sur médias manquants, chemins, liens symboliques et téléchargements.
- [ ] Mesurer les performances avec de véritables ISO volumineuses et une grosse bibliothèque.

## Phase 3 — Client Rust/SDL2

- [x] Configuration YAML, choix de langue (fr/en/es/de) et interface séparée du réseau et des installations.
- [x] Écrans systèmes, catalogue à défilement infini, fiche et file d'attente.
- [x] Indicateurs des ROMs déjà présentes localement, actualisés après installation.
- [x] Catalogue en grille de jaquettes avec noms, navigation bidimensionnelle et chargement asynchrone.
- [x] Vues jaquettes / liste détaillée pour jeux et plateformes, ouverture systématique de la fiche avant toute installation.
- [x] Désinstallation locale depuis la fiche, confirmation par défaut sur Annuler, sauvegarde XML et conservation des médias/sauvegardes.
- [x] Captures d'écran depuis le XML, affichage et installation avec les autres médias.
- [x] Clavier, boutons manette, stick, zone morte, répétition et branchement à chaud.
- [x] Jaquettes avec cache, limites de décodage et image de remplacement.
- [x] Vérification de l'initialisation et du rendu SDL sans écran.
- [x] Recherche globale et par plateforme, saisie clavier et clavier virtuel à la manette.
- [x] Rechargement serveur depuis le client (F5/L1), suivi sans bloquer SDL et actualisation des catalogues.
- [x] Logo `marquee`, fond de fiche `fanart` et proportions naturelles des jaquettes.
- [ ] Filtres avancés dans l'interface ; configuration à l'écran.
- [ ] Police embarquée avec licence de redistribution.
- [ ] Validation manette et lisibilité sur téléviseur et Batocera réel.
- [x] Génération d'archives x86_64/ARM64 et installateur Batocera avec lanceur Ports, police et configuration préservée.

## Phase 4 — Installation de bout en bout

- [x] File séquentielle, progression, débit et annulation.
- [x] Validation des manifestes, chemins, tailles, espace libre et SHA-256.
- [x] Fichiers temporaires sur le même système de fichiers et publication sans écrasement.
- [x] Verrou interprocessus par système, journal et récupération manuelle.
- [x] Fusion XML idempotente, sauvegardes et conservation des champs locaux.
- [x] Refus des XML invalides et détection optimiste des modifications concurrentes.
- [x] Tests réels API Rust → client Rust → fichiers et XML locaux, sans dépendance aux ROMs utilisateur.
- [ ] Valider la procédure sûre d'intégration et de rafraîchissement EmulationStation.

## Phase 5 — Résilience et compatibilité étendue

- [x] Jeux multidisques `.m3u` / `.m3u8` : disques autonomes, organisation identique côté client, contrôles des références et récupération ; playlists imbriquées refusées.
- [ ] Dépendances multi-fichiers `.cue` / `.gdi` / `.ccd` / `.mds` et playlists imbriquées, avec détection des cycles.
- [ ] Reprise HTTP Range validée par révision et file persistante.
- [ ] Gestion des temporaires abandonnés après arrêt brutal et politique de rétention des sauvegardes.
- [ ] Optimisations mesurées sur gros catalogues et petites machines.
- [ ] Authentification et déploiement TLS avant tout accès public.

## Phase 6 — Livraison Batocera

- [x] Documentation de démarrage, configuration et dépannage.
- [x] Contrôles Rust du serveur/client, outils d'installation Python séparés et test SDL en CI.
- [ ] Construction pour l'architecture retenue et vérification des dépendances sur Batocera.
- [ ] Lanceur et intégration au menu de Batocera.
- [ ] Installation puis lancement d'un véritable jeu de test autorisé sur le matériel cible.

## Critères d'acceptation du MVP

Le parcours automatisé installe un fichier de ROM synthétique et son image dans les bons répertoires ; crée ou fusionne le XML ; conserve favoris, statistiques et autres jeux ; ne crée pas de doublon après réinstallation ; refuse un conflit local ; reprend une installation partiellement publiée à partir de son journal.

Le parcours graphique doit encore être validé avec une manette physique et le matériel Batocera cible. La réussite des tests Linux ne vaut pas validation d'un paquet Batocera.
