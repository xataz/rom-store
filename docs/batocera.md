# Installer le client sur Batocera

Le client s'installe dans la partition persistante **`/userdata`**, sans Rust, Cargo,
Docker ni modification du système Batocera. Il apparaît dans **Ports → ROM Store**.
Le serveur Rust reste sur votre serveur/NAS/PC : il n'est pas installé sur Batocera.

## 1. Obtenir l'archive

Deux architectures sont prévues, à choisir selon **le Batocera destinataire** :

| Résultat de `uname -m` sur Batocera | Archive |
|---|---|
| `x86_64` (PC, mini-PC…) | `romstore-client-0.1.0-batocera-x86_64.tar.gz` |
| `aarch64` (système ARM **64 bits**) | `romstore-client-0.1.0-batocera-aarch64.tar.gz` |

Un appareil ARM avec un système 32 bits (`armv7l`, etc.) ne peut pas utiliser l'archive
ARM64. Ces paquets utilisent les bibliothèques de Batocera : prévoir **glibc 2.36+**, SDL2
**2.26+** et un environnement graphique SDL fonctionnel. Ils ne sont pas universels
pour toutes les anciennes versions ni toutes les cartes. L'installateur vérifie
l'architecture et le chargement du binaire avant de modifier une installation existante.
La validation de l'affichage et de la manette sur le matériel réel reste nécessaire.

Il n'y a pas de téléchargement de release publique configuré dans ce dépôt. Pour
produire l'archive depuis un PC Linux équipé de **Docker ou Podman**, à la racine du projet :

```bash
bash scripts/build-batocera.sh --arch x86_64
```

Les fichiers `.tar.gz` et `.tar.gz.sha256` sont écrits dans **`dist/`**.
Le conteneur compile en mode release avec Rust 1.90 et le `Cargo.lock` du projet,
sur Debian Bookworm, afin de ne pas dépendre de la glibc récente du PC de développement.
Une police DejaVu Sans et sa licence sont incluses ; SDL2 et les pilotes graphiques
ne sont pas remplacés.

Pour ARM64, utiliser idéalement un PC/runner ARM64 :

```bash
bash scripts/build-batocera.sh --arch aarch64
```

Sur un hôte d'architecture différente, Docker/Podman doit déjà être configuré pour
l'émulation multiarchitecture (QEMU/binfmt). Le script n'installe rien sur l'hôte.
Options supplémentaires : `--engine docker|podman` et `--output /chemin/de/sortie`.

Sur GitHub, le workflow manuel **Batocera client packages** produit aussi les deux
archives comme *artifacts*, sur des runners natifs séparés. Il ne publie pas de release.

## 2. Copier et installer

Pour un Batocera PC x86_64, copier depuis le PC :

```bash
scp dist/romstore-client-0.1.0-batocera-x86_64.tar.gz* root@batocera:/userdata/system/
ssh root@batocera
```

Remplacer `batocera` par son adresse IP si nécessaire. Sur Batocera :

```bash
cd /userdata/system
sha256sum -c romstore-client-0.1.0-batocera-x86_64.tar.gz.sha256
tar -xzf romstore-client-0.1.0-batocera-x86_64.tar.gz
python3 romstore-client-0.1.0-batocera-x86_64/install.py \
  --server http://192.168.1.10:8000
```

**Remplacer `192.168.1.10:8000` par l'adresse et le port réels du serveur ROM Store**
(par exemple `:6789` si le serveur écoute sur ce port). Ne pas utiliser `127.0.0.1`
si le serveur est sur une autre machine. Le serveur doit écouter sur une adresse
accessible depuis le réseau local, pas seulement sur sa boucle locale.

Pour ARM64, remplacer `x86_64` par `aarch64` dans ces commandes. Il est également
possible de copier les deux fichiers via le partage réseau Batocera, dans `system/`.
Les sommes SHA-256 détectent une copie corrompue, **pas** un éditeur malveillant :
n'installer que des archives provenant d'une source de confiance.

L'installateur :

- vérifie les empreintes des fichiers et l'architecture ;
- teste le chargement du binaire **sans ouvrir de fenêtre ni contacter le serveur** ;
- installe le binaire et la police dans `/userdata/system/romstore/` ;
- crée la configuration YAML en plein écran, avec le français par défaut ;
- ajoute le lanceur `/userdata/roms/ports/ROM Store.sh` ;
- demande une actualisation locale d'EmulationStation via `127.0.0.1:1234/reloadgames`.

Ouvrir ensuite **Ports → ROM Store**. Si l'actualisation automatique n'est pas
disponible, utiliser le menu Batocera **Mettre à jour la liste des jeux**.
L'accès Web public de Batocera n'a pas besoin d'être activé pour cet appel local.
`--no-reload` permet de ne pas demander cette actualisation.

Pour une autre langue dès la première installation, ajouter `--language en`, `es` ou `de`.
Python 3, déjà présent sur les versions récentes de Batocera, suffit à l'installation.
Aucun téléchargement de dépendances n'est effectué par l'installateur sur Batocera.

## Configuration et journaux

| Élément | Emplacement |
|---|---|
| Configuration | `/userdata/system/configs/romstore/config.yaml` |
| Application et police | `/userdata/system/romstore/` |
| ROMs installées | `/userdata/roms/<plateforme>/…` |
| Cache des images | `/userdata/system/cache/romstore/` |
| Journal du client | `/userdata/system/logs/romstore.log` |
| Lanceur Ports | `/userdata/roms/ports/ROM Store.sh` |

Le lanceur transmet explicitement le chemin de configuration et conserve
l'environnement graphique et les contrôleurs fournis par Batocera. Il n'arrête pas
EmulationStation et ne force pas X11 sur les appareils DRM/Wayland.

Pour changer le serveur ou la langue ultérieurement, modifier le YAML, par exemple :

```yaml
server_url: "http://192.168.1.10:6789"
language: fr
frontend: batocera
auto_refresh: true
```

Les autres réglages (`fullscreen`, `games_view`, `systems_view`…) restent ceux du client
habituel. Le champ `font_path` pointe sur la police incluse dans l'application.

Les nouvelles installations activent le refresh automatique des jeux à la fermeture
normale du client : fin des écritures locales, fermeture de SDL, puis demande à
`127.0.0.1:1234/reloadgames`, sans redémarrage forcé. Pour le désactiver, utiliser
`auto_refresh: false`. Les configurations déjà présentes sont conservées : ajouter
les deux options ci-dessus pour activer ce comportement après une mise à jour.
L’option `--no-reload` de l’installateur ne concerne que le refresh effectué pendant
l’installation du paquet, pas celui du client à sa fermeture.

## Mettre à jour

Quitter ROM Store, copier et extraire la nouvelle archive, puis relancer son installateur :

```bash
python3 romstore-client-0.1.0-batocera-x86_64/install.py
```

La version du répertoire peut changer selon l'archive. **La configuration existante
est conservée intégralement**, commentaires compris ; `--server` et `--language` ne la
réécrivent pas lors d'une mise à jour. Le binaire, la police et le lanceur géré sont
actualisés. Les ROMs, médias, `gamelist.xml`, journaux d'installation et caches ne sont
pas supprimés. Un lanceur ou dossier d'application préexistant non géré par cet
installateur est refusé plutôt qu'écrasé.

## Diagnostic

- **Mauvaise architecture** : vérifier `uname -m`, puis choisir l'autre archive.
- **`GLIBC_… not found` ou bibliothèque SDL absente** : la version/plateforme Batocera
  n'est pas compatible avec ce binaire. L'installateur ne remplace pas l'application
  existante ; mettre à jour Batocera ou compiler avec son SDK/sysroot adapté.
- **Retour immédiat au menu Ports** : consulter `/userdata/system/logs/romstore.log`
  et les journaux Batocera `/userdata/system/logs/es_launch_stderr.log` et
  `es_launch_stdout.log`.
- **Pas de connexion** : vérifier l'adresse LAN, le port du serveur et son pare-feu.
- **Pas d'entrée Ports** : actualiser les listes, vérifier les systèmes affichés et
  l'option Batocera de lecture exclusive des gamelists. L'installateur ne modifie
  jamais le `gamelist.xml` de vos ports ni vos réglages EmulationStation.

Le paramètre avancé `--userdata /chemin/absolu` permet de tester l'installateur dans
une arborescence isolée ; il n'est pas nécessaire sur Batocera.
