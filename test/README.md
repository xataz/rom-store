# Petite bibliothèque de démonstration

Ces données sont entièrement synthétiques, sans jeu ni image commerciale.
**Les ROMs sont de simples fichiers texte, non jouables** ; les `.chd` ne sont
pas de véritables images disque. Les PNG sont valides et affichables par le client.

| Plateforme | Jeux | Cas couverts |
|---|---|---|
| NES | Pixel Pilot ; L'île & les étoiles | ROM simple, espaces, accents, apostrophe et `&` dans les chemins |
| SNES | Turbo Garden ; Sans image | Jaquettes et jeu sans média facultatif |
| PlayStation | Orbital Quest | Playlist M3U et deux disques CHD factices dans un sous-dossier |

`library/` suit l'organisation Batocera, avec un `gamelist.xml` par plateforme.
Chaque jeu illustré possède un aperçu paysage et une jaquette portrait dans
`images/`. `system-icons/` contient les trois icônes de plateformes.

Depuis la **racine du projet**, dans deux terminaux :

```bash
make run-server
```

```bash
make run-client
```

L'interface web du serveur est disponible sur <http://127.0.0.1:8000/>.
La configuration `test/client.yaml` installe les jeux dans
`.romstore/test/installed`, jamais dans la bibliothèque source. Base SQLite et
cache sont également isolés dans `.romstore/test/` (ignoré par Git).
Le client utilise la détection automatique de police du système.

Les données sont déjà fournies : Python n'est pas nécessaire pour les utiliser.
Pour les recréer à l'identique, lancer `make fixtures` avec Python 3.
**Cette commande réécrit les fichiers de démonstration connus** : sauvegarder toute
modification manuelle de ces fichiers auparavant. Elle ne nettoie pas le dossier
et ne touche ni à la configuration client ni aux fichiers installés.
