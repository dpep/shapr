# typed: true
class A
  extend T::Sig

  sig { returns(Integer) }
  def total
    1
  end
end
