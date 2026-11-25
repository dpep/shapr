# typed: true
class A
  extend T::Sig

  def initialize
    @cache = T.let(nil, T.nilable(String))
  end
end
