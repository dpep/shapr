# typed: true
class A
  extend T::Sig

  T::Sig::WithoutRuntime.sig { returns(Integer) }
  def total
    1
  end
end
