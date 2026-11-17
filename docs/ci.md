# CI/CD Codeberg / Forgejo Actions

Le workflow [`.forgejo/workflows/ci.yml`](../.forgejo/workflows/ci.yml) cible
**Linux x86_64 uniquement**. Il n'utilise ni runner ARM64 ni compilation croisée.
Les workflows GitHub existants sont conservés ; la présence du dossier `.forgejo`
permet à Forgejo d'utiliser ses propres workflows à la place de `.github`.

## Préparer Codeberg et le runner

1. Activer **Actions** dans les paramètres du dépôt si nécessaire.
2. Vérifier que le runner x86_64 est connecté et exécute les jobs dans des
   **conteneurs Docker** (Podman compatible est également possible).
3. Le workflow utilise `runs-on: docker`. Remplacer `docker` par le label réel du
   runner dans le fichier YAML si nécessaire, ou déclarer un label tel que
   `docker:docker://node:22-bookworm` dans la configuration du runner.
   Un label `ubuntu-latest` ne correspond pas automatiquement à `docker`.
4. Le runner doit pouvoir télécharger l'image `node:22-bookworm`, les actions
   depuis `code.forgejo.org`, Rust et les dépendances Debian/Cargo. Son service de
   cache doit être configuré et accessible depuis les conteneurs pour l'action cache.

L'image de job est explicitement définie : **Debian 12 Bookworm, Node 22**, avec
installation de **Rust 1.90.0**, Clippy, rustfmt et, pour le client, SDL2. L'installation des paquets
s'effectue en root **dans le conteneur**, pas sur l'hôte ; aucun mode privilégié
ni montage du socket Docker dans le job n'est nécessaire.

Le jeton temporaire `${{ secrets.GITHUB_TOKEN }}` est fourni automatiquement par
Forgejo et utilisé pour créer la release dans **le même dépôt**. Aucun jeton
personnel n'est nécessaire dans la configuration habituelle. Si l'API renvoie
403, vérifier la politique de permissions du jeton Actions de l'instance/dépôt.
Ne jamais placer de jeton directement dans le YAML ou les scripts.

## Ce qui s'exécute

Les deux composants ont des versions et des releases indépendantes :

| Événement | Composants testés et compilés | Publication |
|---|---|---|
| Push de branche / pull request | Client et serveur, avec E2E | Non |
| Push de `client-vX.Y.Z` | Client uniquement, avec tests SDL | Release client |
| Push de `server-vX.Y.Z` | Serveur uniquement, avec tests WebUI | Release serveur |
| Lancement manuel | Choix `all`, `client` ou `server` | Non |

Les anciens tags communs `vX.Y.Z` ne publient plus de release. Les autres tags
exécutent les vérifications des deux composants, sans publication.

Pour les composants sélectionnés :

- vérification de l'architecture x86_64 ;
- formatage Rust et Clippy, avec avertissements traités comme erreurs ;
- tests des outils de release, et du JavaScript si le serveur est sélectionné ;
- tests unitaires et d'intégration Rust (`make test-client`, `make test-server` ou `make test`) ;
- compilation en release (`make client`, `make server` ou `make all`) ;
- tests E2E ignorés par défaut, avec le véritable client de release, **si les deux
  composants sont sélectionnés** ;
- test de rendu SDL sans écran et test de densité du texte si le client est sélectionné.

Une release serveur ne nécessite donc ni compilation du client ni SDL2. Une release
client ne nécessite pas de compiler le serveur. Les tests de compatibilité E2E sont
conservés sur les branches/PR et les lancements manuels `all`, mais ne bloquent pas
une release indépendante sur la compilation de l'autre composant.

Les étapes s'arrêtent au premier échec. Les dépendances téléchargées par Cargo
sont mises en cache ; les répertoires `target` ne le sont pas. Aucun ancien outil
de packaging Batocera absent du dépôt n'est appelé.

Sur un **push de tag `client-v…` ou `server-v…`**, après ces vérifications :

1. Vérification du tag : il doit correspondre à la version du **seul composant ciblé**.
2. Vérification du binaire ELF64 x86_64, puis création de son archive.
3. Création d'une release en **brouillon** et téléversement de l'archive et de `SHA256SUMS`.
4. Publication uniquement une fois tous les fichiers envoyés. Un tag avec suffixe
   comme `client-v0.2.0-rc.1` produit une prérelease.

Un lancement manuel, même sur un tag, ne publie pas de release. Une pull request
ne déclenche jamais les étapes de publication ; le checkout ne conserve pas ses
identifiants Git. Utiliser uniquement des runners isolés pour du code non fiable,
et ne pas approuver aveuglément les workflows provenant de forks.

## Publier une version

Mettre à jour `package.version` dans le `Cargo.toml` du composant concerné,
puis actualiser et committer **son** `Cargo.lock` avec le manifeste. La version
de l'autre composant reste inchangée. Par exemple, un client `0.2.0` peut coexister
avec un serveur `0.1.3` ; leur compatibilité dépend du contrat de l'API, pas de
l'égalité de leurs versions. L'interface du client affiche automatiquement sa
version de compilation, issue de `client/Cargo.toml`, dans un petit encadré d'en-tête.
Le tag doit pointer sur le commit contenant également le workflow et les scripts.
Exemple pour publier uniquement le client `0.1.0` :

```bash
git tag -a client-v0.1.0 -m 'ROM Store Client 0.1.0'
git push origin client-v0.1.0
```

Après réussite, la release Codeberg contient :

```text
romstore-client-0.1.0-linux-x86_64.tar.gz
SHA256SUMS
```

Pour le serveur, utiliser par exemple `server-v0.1.3`, avec `package.version = "0.1.3"`
dans `server/Cargo.toml`. Sa release contiendra uniquement
`romstore-server-0.1.3-linux-x86_64.tar.gz` et son propre `SHA256SUMS`.
Les archives locales sont isolées dans `dist/client-v…/` et `dist/server-v…/`.

Chaque archive contient un dossier versionné avec le binaire et la documentation ;
celle du client inclut aussi `config.example.yaml`. Aucune configuration privée,
ROM, base SQLite ni donnée de cache n'est incluse.

Une release existante n'est **jamais écrasée**. Si un envoi échoue, elle reste en
brouillon : supprimer manuellement ce brouillon incomplet sur Codeberg, sans
supprimer le tag, puis relancer le job. Ne pas supprimer une release déjà publiée
pour remplacer silencieusement ses binaires ; utiliser un nouveau numéro de version.

## Utiliser les archives

Télécharger l'archive et le `SHA256SUMS` de **la même release** dans un dossier :

```bash
sha256sum -c SHA256SUMS
tar -xzf romstore-client-0.1.0-linux-x86_64.tar.gz
```

Procéder de même pour une release serveur, de préférence dans un dossier distinct
pour ne pas confondre les deux fichiers `SHA256SUMS`.

Ce sont des binaires **dynamiques**, compilés sur Debian 12 : cible de base
**glibc 2.36+**, avec les bibliothèques d'exécution système habituelles.
Pour le client, installer **SDL2 2.26+** et une police TrueType, par exemple sur Debian :

```bash
sudo apt-get install libsdl2-2.0-0 fonts-dejavu-core
```

Copier `config.example.yaml` vers un fichier personnel et adapter ses chemins
avant de lancer `romstore-client --config /chemin/config.yaml`. Le serveur utilise
les options décrites dans `docs/server.md`. Le serveur n'a pas besoin de SDL2.

Ces archives ne sont **pas des installateurs Batocera** : elles ne créent ni entrée
Ports ni configuration automatiquement. La compatibilité avec une version donnée
de Batocera doit être vérifiée séparément (glibc/SDL2 et matériel).

## Vérification locale des outils de release

Python **3.11+** est requis uniquement pour ces outils CI, pas pour les binaires :

```bash
python3 -m unittest discover -s scripts/ci -p 'test_*.py' -v
make client
python3 scripts/ci/package.py client-v0.1.0
# Ou, indépendamment :
make server
python3 scripts/ci/package.py server-v0.1.0
```

Ces commandes écrivent dans `dist/client-v0.1.0/` ou `dist/server-v0.1.0/`, ignorés
par Git, sans contacter Codeberg. Adapter le tag à la version du manifeste concerné.
Le script de publication n'est appelé que par l'étape dédiée du workflow.
