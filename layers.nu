#!/usr/bin/env nu

# Hypothesis : layers are fetched in the same order as in the recipe

use std/log

# # FUTURE: get image from remote registry
# # const image_name = "niri"
# # const repo = "benoitlx"
# # let layers = skopeo inspect docker://ghcr.io/($repo)/($image_name):latest | from json | get Layers

# # $scope | table # --expand # --abbreviated 3

def main [] {
  print "Command: clear, inspect"
}

def "main clear" [] {
  rm -rf /tmp/image-scope/
}

def "main inspect" [image_name: string] {
  let hash = podman inspect $image_name | from json | get Digest | str replace "sha256:" '' | get 0

  let working_dir = $"/tmp/image-scope/.cache/($hash)"
  let image = $"($working_dir)/image.tar"
  let meta = $"($working_dir)/.layer_scope.json"

  if not ($image | path exists) {
    mkdir $working_dir
    podman save $image_name -o $image
  } else {
    log info $"(ansi magenta)Working with cached image: ($image)(ansi reset)"
  }

  let layers = if ($meta | path exists) {
    cat $meta | from json
  } else {
    tar -C $working_dir -xf $image "manifest.json"

    let full_layers = cat $"($working_dir)/manifest.json" | from json | get Layers.0 | parse "{blobs}.tar" | get blobs

    $full_layers | to json | save $meta

    $full_layers
  }
  log info $"(ansi green)Processing ($layers | length) layers (ansi reset)"

  "[]" | save -f $meta

  $layers | process_layers $working_dir $image $meta | to json | save -f "packages-map.json"
}

def process_layers [working_dir: string, image: string, meta: string] {
  mut scope = []

  for blob in $in {
    let blob_shortname = $blob | str substring 0..6

    log info $"(ansi green) Processing blob ($blob) (ansi reset)"

    let blob_dir = $"($working_dir)/blobs/($blob)"
    let tar_blob = $"($blob_dir)/($blob).tar"

    mkdir $blob_dir

    tar -C $blob_dir -xf $image $"($blob).tar"

    try {
      tar -C $blob_dir -xf $tar_blob "usr/share/rpm/rpmdb.sqlite"
      cat $meta | from json | append $blob | to json | save -f $meta
    } catch {
      log warning $"No rpm database in layer ($blob), removing ($blob_dir)"
      rm -rf $blob_dir
      continue
    }

    let dbpath = $"--dbpath=($blob_dir)/usr/share/rpm/"
    log info $"(ansi blue)Querying rpm database: ($dbpath)(ansi reset)"
    let packages = rpm $dbpath -qa | lines

    for package in $packages {
      log info $"(ansi purple)Processing package ($package)(ansi reset)"

      if not ($package in ($scope | get 'fullname')) {
        $scope = $scope | append (
          rpm $dbpath -q --json --info $package
            | from json
            | select --optional Name Version Release Arch Installtime Group Size License Sourcerpm Buildtime Buildhost Packager Vendor Url Bugurl Summary Description
            | insert "introduced_in" $blob_shortname
            | insert "dep" (
                rpm $dbpath -qR $package
                  | lines
                  | par-each -k {|cap| (rpm -q --qf '%{NAME}\n' --whatprovides ($cap | split row ' ').0) | default null}
                  | where (str contains "no package provides" | not $in)
                  | where (str contains "\n" | not $in)
                  | uniq
              )
            | insert "dropped" false
            | insert 'fullname' $package
            | merge {Summary: ($in.Summary | str join)}
            | merge {Description: ($in.Description | str join)}
        )
      }
    }
  }

  $scope
}
