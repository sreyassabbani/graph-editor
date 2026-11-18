{ pkgs, ... }:
{
  packages = with pkgs; [
    typst
  ];

  languages.rust.enable = true;
}
