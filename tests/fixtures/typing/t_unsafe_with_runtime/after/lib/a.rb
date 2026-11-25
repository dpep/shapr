# typed: true
class A
  extend T::Sig

  def run(x)
    T.unsafe(x).go
  end
end
