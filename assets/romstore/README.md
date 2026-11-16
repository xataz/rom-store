# Icône et entrée de lancement ROM Store

- `romstore.png` : icône 512 × 512 pixels, cartouche rétro bleu nuit et menthe.
- `romstore.svg` : source vectorielle redimensionnable, sans police externe.
- `gamelist.xml` : entrée pour Batocera / EmulationStation.
- `gamelist.esde.xml` : entrée pour ES-DE, sans balises média.

Ces fichiers supposent un lanceur exécutable nommé **`ROM Store.sh`** dans le
système `ports`, comme celui créé par l’installateur Batocera. Pour un autre nom
ou système, adapter les chemins et le nom du média ES-DE en conséquence.
Le lanceur doit déjà fonctionner sur la machine cible : le XML ne le crée pas.

## Batocera

Disposition attendue :

```text
/userdata/roms/ports/
├── ROM Store.sh
├── gamelist.xml
└── images/
    └── romstore.png
```

Copier `romstore.png` dans `images/`, puis ajouter le nœud `<game>` de
`gamelist.xml` au `<gameList>` existant. Si aucune gamelist n’existe, utiliser le
fichier complet. **Ne pas remplacer une gamelist existante contenant d’autres
jeux**, et ne pas ajouter de doublon si une entrée `./ROM Store.sh` existe déjà :
modifier cette entrée. Actualiser ensuite la liste des jeux dans Batocera.

## ES-DE

Avec les chemins habituels et le système `ports` :

```text
~/ROMs/ports/ROM Store.sh
~/ES-DE/gamelists/ports/gamelist.xml
~/ES-DE/downloaded_media/ports/miximages/ROM Store.png
~/ES-DE/downloaded_media/ports/covers/ROM Store.png
```

Fermer ES-DE avant de modifier sa bibliothèque. Ajouter le nœud `<game>` de
`gamelist.esde.xml` au gamelist du système, ou utiliser le fichier complet
renommé `gamelist.xml` si celui-ci n’existe pas. Copier `romstore.png` sous le nom
**`ROM Store.png`** dans `miximages/` et, si souhaité, dans `covers/`.
ES-DE retrouve les médias par le nom du lanceur sans son extension, pas par
des balises dans le XML. Adapter les racines si elles ont été personnalisées,
puis relancer ES-DE.

## Régénérer le PNG

Depuis la racine du dépôt, avec ImageMagick :

```sh
magick -background none -density 192 assets/romstore/romstore.svg -resize 512x512 assets/romstore/romstore.png
```

Ces ressources sont fournies séparément ; l’installateur Batocera n’est pas
modifié pour les copier automatiquement ni fusionner le gamelist.
