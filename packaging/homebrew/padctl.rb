# Source-only HEAD formula. No binary release or published tap is provided.
# Build instructions in README.md are the validated installation path.
class Padctl < Formula
  desc "GPU-aware adaptive control of a Razer Laptop Cooling Pad"
  homepage "https://github.com/marc-shade/mac-cooling-control"
  license "MIT"
  head "https://github.com/marc-shade/mac-cooling-control.git", branch: "main"

  depends_on "rust" => :build

  def install
    system "cargo", "install", *std_cargo_args
    generate_completions_from_executable(bin/"padctl", "completions")
    (buildpath/"padctl.1").write Utils.safe_popen_read(bin/"padctl", "manpage")
    man1.install "padctl.1"
  end

  test do
    assert_match "padctl", shell_output("#{bin}/padctl --version")
  end
end
