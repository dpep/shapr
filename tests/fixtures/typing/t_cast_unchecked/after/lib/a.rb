# typed: true
class A
  extend T::Sig

  def run(x)
    T.cast(x, Runner, checked: false).go
  end
end
