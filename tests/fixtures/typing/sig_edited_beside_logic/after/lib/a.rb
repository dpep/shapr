# typed: true
class A
  extend T::Sig

  sig { returns(String) }
  def total
    "1"
  end
end
