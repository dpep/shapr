# typed: true
class A
  extend T::Sig

  sig do
    params(x: Integer).returns(Integer)
  end
  def twice(x)
    x * 2
  end
end
