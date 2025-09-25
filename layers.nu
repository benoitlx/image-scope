#!/usr/bin/env nu

# Hypothesis : layers are fetched in the same order as in the recipe

use std/log

# FUTURE: get image from remote registry
# const image_name = "niri"
# const repo = "benoitlx"
# let layers = skopeo inspect docker://ghcr.io/($repo)/($image_name):latest | from json | get Layers

# TODO
# make a function get layers

const local_image = "localhost/niri"
const tar_dir = "image_dir"
const tar_image = $"($tar_dir)/image.tar"

# save local image to an archive
if not ($tar_image | path exists) {
  mkdir $tar_dir
  podman save $local_image -o $tar_image
}
tar -C $tar_dir -xf $tar_image "manifest.json" # $"($tar_dir)/blobs/"

let layers = cat $"($tar_dir)/manifest.json" | from json | get Layers.0 | parse "{blobs}.tar" | get blobs

mut scope = []
mut layer_rec = {}

for blob in $layers {
  let blob_shortname = $blob | str substring 0..6

  log info $"(ansi blue) Processing blob ($blob) (ansi reset)"

  let blob_dir = $"($tar_dir)/blobs/($blob)"
  let tar_blob = $"($blob_dir)/($blob).tar"

  mkdir $blob_dir

  tar -C $blob_dir -xf $tar_image $"($blob).tar"

  try {
    tar -C $blob_dir -xf $tar_blob "usr/share/rpm/rpmdb.sqlite"
  } catch {
    log info $"No rpm database in layer ($blob)"
    continue
  }

  let dbpath = $"--dbpath=/home/bleroux/Documents/rpm-layer-scope/($blob_dir)/usr/share/rpm/"
  log info $"(ansi green)Querying rpm database: ($dbpath)(ansi reset)"
  let packages = rpm $dbpath -qa | lines

  for package in $packages {
    if not ($package in ($scope | get 'fullname')) {
      $scope = $scope | append (
        do {
          # TODO
          # - fix field 'Description'
          # - parse dependencies

          let infos = ((rpm $dbpath -q --info $package) | parse --regex '(?s)^(?P<key>.*?) *:(?P<value>.*?)$' | reduce -f {} {|it, acc| $acc | upsert $it.key $it.value })
          $infos
            | insert "introduced_in" $blob_shortname
            | insert "dep" ((rpm $dbpath -qR $package) | lines),
            | insert "dropped" false
            | insert 'fullname' $package
        }
      )
    }
  }
}

$scope | to json | save -f system.txt
# $scope | table # --expand # --abbreviated 3
